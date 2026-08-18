use std::path::{Path, PathBuf};

fn profile_target_dirs(root: &Path, debug_build: bool) -> [PathBuf; 2] {
    if debug_build {
        // `just dev` builds fresh debug sidecars; never prefer stale release output.
        [root.join("target/debug"), root.join("target/release")]
    } else {
        [root.join("target/release"), root.join("target/debug")]
    }
}

pub(super) fn ordered_command_search_dirs(
    workspace_root: &Path,
    current_dir: Option<&Path>,
    executable_parent: Option<&Path>,
    debug_build: bool,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // A packaged release must use the sidecars shipped beside its executable.
    // Build paths can exist on the test machine and can contain an older
    // `buzz-acp`; they must not override the package during an upgrade.
    if !debug_build {
        dirs.extend(executable_parent.map(Path::to_path_buf));
    }

    dirs.extend(profile_target_dirs(workspace_root, debug_build));
    if let Some(current_dir) = current_dir {
        dirs.extend(profile_target_dirs(current_dir, debug_build));
    }

    if debug_build {
        dirs.extend(executable_parent.map(Path::to_path_buf));
    }

    dirs.into_iter().fold(Vec::new(), |mut unique, dir| {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
        unique
    })
}
