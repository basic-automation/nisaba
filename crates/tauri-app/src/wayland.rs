//! WebKitGTK's DMA-BUF renderer crashes on some Wayland compositors (seen on
//! Hyprland with NVIDIA): the window dies at startup with "Error 71 (Protocol
//! error) dispatching to Wayland display". Turning that renderer off is
//! WebKitGTK's own escape hatch, so on a Wayland session the app sets
//! `WEBKIT_DISABLE_DMABUF_RENDERER=1` before GTK starts, unless the user has
//! already set it (to any value, including `0` to opt back in).

const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Apply the workaround. Must run first in `main`, before any thread exists,
/// because it writes the process environment.
pub fn apply_webkit_workaround() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let get = |name: &str| std::env::var_os(name);
    if needs_workaround(
        get(VAR).is_some(),
        get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        get("XDG_SESSION_TYPE").as_deref() == Some("wayland".as_ref()),
    ) {
        std::env::set_var(VAR, "1");
    }
}

fn needs_workaround(already_set: bool, wayland_display: bool, wayland_session: bool) -> bool {
    !already_set && (wayland_display || wayland_session)
}

#[cfg(test)]
mod tests {
    use super::needs_workaround;

    #[test]
    fn set_only_on_wayland_and_never_over_the_users_choice() {
        assert!(needs_workaround(false, true, false));
        assert!(needs_workaround(false, false, true));
        assert!(
            !needs_workaround(false, false, false),
            "X11 keeps the default renderer"
        );
        assert!(
            !needs_workaround(true, true, true),
            "an existing value wins"
        );
    }
}
