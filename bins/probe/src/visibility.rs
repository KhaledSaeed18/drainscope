//! Which cgroups a caller may see: everything except other users' slices (ADR 0006).

/// Whether a caller with `uid` (`None`: unknown) may see counters of the cgroup at `path`
/// (relative to the cgroupfs root).
#[must_use]
pub fn visible(path: &str, uid: Option<u32>) -> bool {
    let Some(rest) = path.strip_prefix("user.slice") else {
        return true;
    };
    let Some(rest) = rest.strip_prefix('/') else {
        // `user.slice` itself (or a sibling like `user.slicex`, which systemd never makes).
        return rest.is_empty();
    };
    let Some(slice) = rest.split('/').next() else {
        return false;
    };
    uid.is_some_and(|uid| slice == format!("user-{uid}.slice"))
}

#[cfg(test)]
mod tests {
    use super::visible;

    #[test]
    fn hides_other_users() {
        let own =
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-firefox-1.scope";
        assert!(visible(own, Some(1000)));
        assert!(!visible(own, Some(1001)));
        assert!(!visible(own, None));
        assert!(!visible(
            "user.slice/user-0.slice/session-3.scope",
            Some(1000)
        ));
        assert!(!visible("user.slice/user-10000.slice", Some(1000)));
    }

    #[test]
    fn shows_the_system() {
        for path in [
            "",
            "system.slice/NetworkManager.service",
            "init.scope",
            "user.slice",
            "machine.slice",
        ] {
            assert!(visible(path, Some(1000)), "{path}");
            assert!(visible(path, None), "{path}");
        }
    }
}
