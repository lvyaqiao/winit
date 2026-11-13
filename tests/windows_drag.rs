#![cfg(target_os = "windows")]

use std::sync::{
    atomic::{AtomicBool, AtomicIsize, Ordering},
    mpsc, Arc, OnceLock,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        CallWindowProcW, DestroyWindow, PostMessageW, RegisterWindowMessageA, SetWindowLongPtrW,
        GWLP_WNDPROC, HTNOWHERE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCLBUTTONDOWN,
    },
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    platform::{pump_events::EventLoopExtPumpEvents, windows::EventLoopBuilderExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{ResizeDirection, Window, WindowId},
};

#[link(name = "user32")]
extern "system" {
    fn CreateDesktopW(
        name: *const u16,
        device: *const u16,
        mode: *const u8,
        flags: u32,
        access: u32,
        security: *const u8,
    ) -> isize;
    fn SetThreadDesktop(desktop: isize) -> i32;
    fn CloseDesktop(desktop: isize) -> i32;
}

static ORIGINAL: AtomicIsize = AtomicIsize::new(0);
static PACKETS: OnceLock<mpsc::Sender<(usize, isize)>> = OnceLock::new();

unsafe extern "system" fn observe(window: HWND, message: u32, mut w: WPARAM, l: LPARAM) -> LRESULT {
    let drag = unsafe { RegisterWindowMessageA(b"Winit::StartWindowDrag\0".as_ptr()) };
    if message == drag || message == WM_NCLBUTTONDOWN {
        let _ = PACKETS.get().unwrap().send((w, l));
        // Baseline winit sends this directly. Prevent its modal loop even on failure.
        if message == WM_NCLBUTTONDOWN {
            w = HTNOWHERE as _;
        }
    }
    unsafe {
        CallWindowProcW(
            std::mem::transmute(ORIGINAL.load(Ordering::Relaxed)),
            window,
            message,
            w,
            l,
        )
    }
}

#[derive(Clone, Copy, Debug)]
enum Request {
    Ping,
    Destroy,
    PressAtPoint,
}
struct Handler {
    window: HWND,
    ack: mpsc::Sender<()>,
    positions: mpsc::Sender<Option<(f64, f64)>>,
    cursor: Option<(f64, f64)>,
}
impl ApplicationHandler<Request> for Handler {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y))
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                let _ = self.positions.send(self.cursor);
            }
            _ => {}
        }
    }
    fn user_event(&mut self, _: &ActiveEventLoop, request: Request) {
        if matches!(request, Request::Destroy) {
            unsafe {
                DestroyWindow(self.window);
            }
        }
        if matches!(request, Request::PressAtPoint) {
            let coordinates = 173 | (47 << 16);
            unsafe {
                PostMessageW(self.window, WM_LBUTTONDOWN, 1, coordinates);
                PostMessageW(self.window, WM_LBUTTONUP, 0, coordinates);
            }
        }
        let _ = self.ack.send(());
    }
}

#[test]
fn canceled_native_drags_release_the_request_and_keep_the_message_pump_alive() {
    let (packets_tx, packets) = mpsc::channel();
    PACKETS.set(packets_tx).unwrap();
    let (ready_tx, ready) = mpsc::channel();
    let (ack_tx, ack) = mpsc::channel();
    let (position_tx, positions) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let worker = std::thread::spawn(move || {
        let name: Vec<u16> = format!("RguiDragRegression{}", std::process::id())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let desktop = unsafe {
            CreateDesktopW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0x000f01ff,
                std::ptr::null(),
            )
        };
        assert_ne!(desktop, 0, "create isolated desktop");
        assert_ne!(unsafe { SetThreadDesktop(desktop) }, 0);
        {
            let mut events = EventLoop::<Request>::with_user_event()
                .with_any_thread(true)
                .build()
                .unwrap();
            #[allow(deprecated)]
            let window = Arc::new(
                events
                    .create_window(
                        Window::default_attributes()
                            .with_visible(false)
                            .with_decorations(false),
                    )
                    .unwrap(),
            );
            let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() else {
                unreachable!()
            };
            let hwnd = handle.hwnd.get();
            let old =
                unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, observe as *const () as isize) };
            assert_ne!(old, 0);
            ORIGINAL.store(old, Ordering::Relaxed);
            ready_tx
                .send((window.clone(), events.create_proxy()))
                .unwrap();
            let mut handler = Handler {
                window: hwnd,
                ack: ack_tx,
                positions: position_tx,
                cursor: None,
            };
            let deadline = Instant::now() + Duration::from_secs(8);
            while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
                events.pump_app_events(Some(Duration::from_millis(10)), &mut handler);
            }
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, old);
            }
        }
        // OLE can retain thread-owned helper windows until thread exit.
        desktop
    });
    let (window, proxy) = ready.recv_timeout(Duration::from_secs(5)).unwrap();
    // Request from the input desktop; processing takes place on an inactive desktop,
    // so no physical press can be taken over and no user input is injected.
    for direction in [
        None,
        Some(ResizeDirection::West),
        None,
        Some(ResizeDirection::SouthEast),
    ] {
        match direction {
            None => window.drag_window().unwrap(),
            Some(direction) => window.drag_resize_window(direction).unwrap(),
        }
        let (_, coordinates) = packets
            .recv_timeout(Duration::from_secs(1))
            .expect("subsequent drag was suppressed by a stale request");
        assert_eq!(
            coordinates, coordinates as i32 as isize,
            "mouse payload must be packed coordinates, not a 64-bit address"
        );
        proxy.send_event(Request::Ping).unwrap();
        ack.recv_timeout(Duration::from_secs(1))
            .expect("drag blocked the message pump");
    }
    proxy.send_event(Request::PressAtPoint).unwrap();
    ack.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        positions.recv_timeout(Duration::from_secs(1)).unwrap(),
        Some((173.0, 47.0)),
        "button input must observe its own coordinates even without WM_MOUSEMOVE"
    );
    proxy.send_event(Request::Destroy).unwrap();
    ack.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        window.drag_window().is_err(),
        "failed PostMessage must be reported"
    );
    assert!(
        window.drag_window().is_err(),
        "failed post must not leave a pending drag"
    );
    drop(window);
    stop.store(true, Ordering::Relaxed);
    let desktop = worker.join().unwrap();
    assert_ne!(unsafe { CloseDesktop(desktop) }, 0);
}
