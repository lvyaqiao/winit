# RGUI 0.30 maintenance line

This branch starts at upstream winit 0.30.13, commit
`e9809ef54b18499bb4f2cac945719ecc2a61061b`. The package name, version and
Apache-2.0 license are retained.

Local patches cover signed native-drag coordinates, owner-thread capture
handoff and cancellation, and event-time button positions. The live mixed-DPI
rectangle fix is backported from upstream PR #4694, commit
`39c4009c41e5977d779cf8a365792c8153ca77c5`.

`dpi` uses its published dependency so Git consumers keep the same type source
identity as the released winit package. Windows regression coverage is in
`tests/windows_drag.rs`; signed coordinate coverage is in the Win32 module.

Consumers pin a full commit with a root `[patch.crates-io]`. Maintain patches
here; downstream repositories do not keep editable vendor copies.
