//! Game dumps packed as a SquashFS image (`<name>.squashfs`, as Batocera packs them): the
//! `<gameid>.windowsloader` files at the image's root read without mounting it, and the image
//! mounted read-only with an overlay for the game's writes (settings, saves), as Batocera's
//! configgen does: `<saves>/<image folder name>/<image name>/{upper,work}` (Batocera:
//! `/userdata/saves/<system>/<image name>`, the same save data).
//!
//! As root: kernel squashfs (loop) and overlayfs mounts. Else FUSE: squashfuse and
//! fuse-overlayfs.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use backhand::{FilesystemReader, InnerNode};

use crate::config::DUMP_EXT;

pub const EXT: &str = "squashfs";

pub fn is_image(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// A `<gameid>.windowsloader` file of an image: the game id and its executable path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpFile {
    pub id: String,
    pub exe: String,
}

/// The `.windowsloader` files at the root of an image (its metadata only, no mount).
pub fn dump_files(image: &Path) -> Result<Vec<DumpFile>> {
    let file = File::open(image).with_context(|| format!("opening {}", image.display()))?;
    let fs = FilesystemReader::from_reader(BufReader::new(file))
        .map_err(|e| anyhow::anyhow!("{}: not a SquashFS image ({e})", image.display()))?;
    let mut out = Vec::new();
    for node in fs.files() {
        let path = &node.fullpath;
        let at_root = path.parent().is_some_and(|p| p == Path::new("/"));
        let ext = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(DUMP_EXT));
        let InnerNode::File(f) = &node.inner else { continue };
        if !at_root || !ext {
            continue;
        }
        let mut text = String::new();
        fs.file(f).reader().take(64 * 1024).read_to_string(&mut text).with_context(|| format!("reading {}", path.display()))?;
        let exe = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or_default();
        let id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        out.push(DumpFile { id, exe: exe.to_string() });
    }
    Ok(out)
}

/// The image's only game id.
pub fn game_id(image: &Path) -> Result<String> {
    let mut files = dump_files(image)?;
    match files.len() {
        1 => Ok(files.remove(0).id),
        0 => bail!("{}: no <gameid>.{DUMP_EXT} file at the image's root", image.display()),
        _ => bail!("{}: several .{DUMP_EXT} files: {:?}", image.display(), files.iter().map(|f| &f.id).collect::<Vec<_>>()),
    }
}

/// An image mounted with its overlay, unmounted on drop.
pub struct Mount {
    lower: PathBuf,
    merged: PathBuf,
    fuse: bool,
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Mounts and overlay directories: `$XDG_RUNTIME_DIR/wal-squashfs/<image name>`.
fn mount_base(image: &Path) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("wal-{}", unsafe { libc::getuid() })));
    runtime.join("wal-squashfs").join(image.file_stem().unwrap_or_default())
}

/// The image's save directory (overlay upper and work): `<saves>/<image folder>/<image name>`.
pub fn save_dir(image: &Path, saves: &Path) -> PathBuf {
    let folder = image.parent().and_then(Path::file_name).unwrap_or_default();
    saves.join(folder).join(image.file_stem().unwrap_or_default())
}

/// `lowerdir=...,upperdir=...,workdir=...` (commas in paths escaped).
fn overlay_options(lower: &Path, upper: &Path, work: &Path) -> String {
    let esc = |p: &Path| p.display().to_string().replace(',', "\\,");
    format!("lowerdir={},upperdir={},workdir={}", esc(lower), esc(upper), esc(work))
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("running {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!("{:?} failed ({}): {}", cmd.get_program(), out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn unmount(dir: &Path, fuse: bool) {
    if !dir.exists() {
        return;
    }
    // a process the game left can keep it busy for a moment: detach it lazily then
    let ok = |c: &mut Command| c.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if fuse {
        for bin in ["fusermount3", "fusermount"] {
            if ok(Command::new(bin).arg("-u").arg(dir)) || ok(Command::new(bin).arg("-uz").arg(dir)) {
                return;
            }
        }
    } else if !ok(Command::new("umount").arg(dir)) {
        ok(Command::new("umount").arg("-l").arg(dir));
    }
}

impl Mount {
    /// Mounts `image` read-only and an overlay over it whose writes go to
    /// `save_dir(image, saves)`.
    pub fn new(image: &Path, saves: &Path) -> Result<Mount> {
        let image = std::path::absolute(image)?;
        let base = mount_base(&image);
        let (lower, merged) = (base.join("lower"), base.join("merged"));
        let save = save_dir(&image, saves);
        let (upper, work) = (save.join("upper"), save.join("work"));
        let fuse = !is_root();
        // left mounted by a launcher that was killed
        unmount(&merged, fuse);
        unmount(&lower, fuse);
        for d in [&lower, &merged, &upper, &work] {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let mount = Mount { lower, merged, fuse };
        let options = overlay_options(&mount.lower, &upper, &work);
        if fuse {
            run(Command::new("squashfuse").arg(&image).arg(&mount.lower))
                .context("mounting the image (install squashfuse, or run as root)")?;
            run(Command::new("fuse-overlayfs").arg("-o").arg(&options).arg(&mount.merged))
                .context("mounting its overlay (install fuse-overlayfs, or run as root)")?;
        } else {
            run(Command::new("mount").args(["-t", "squashfs", "-o", "ro,loop"]).arg(&image).arg(&mount.lower))
                .context("mounting the image")?;
            run(Command::new("mount").args(["-t", "overlay", "overlay", "-o"]).arg(&options).arg(&mount.merged))
                .context("mounting its overlay")?;
        }
        eprintln!("launcher: {} mounted on {} (writes in {})", image.display(), mount.merged.display(), save.display());
        Ok(mount)
    }

    /// The game dump (writable).
    pub fn dir(&self) -> &Path {
        &self.merged
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        unmount(&self.merged, self.fuse);
        unmount(&self.lower, self.fuse);
        let _ = std::fs::remove_dir(&self.merged);
        let _ = std::fs::remove_dir(&self.lower);
        // <image name>, then wal-squashfs when no other image is mounted
        for dir in self.lower.ancestors().skip(1).take(2) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_dir_as_batocera() {
        let d = save_dir(Path::new("/userdata/roms/typex/Valve Limit R.squashfs"), Path::new("/userdata/saves"));
        assert_eq!(d, Path::new("/userdata/saves/typex/Valve Limit R"));
    }

    #[test]
    fn overlay_options_escape_commas() {
        let o = overlay_options(Path::new("/a,b"), Path::new("/u"), Path::new("/w"));
        assert_eq!(o, "lowerdir=/a\\,b,upperdir=/u,workdir=/w");
    }

    /// `WAL_TEST_IMAGE=<image.squashfs>`: its .windowsloader files.
    #[test]
    fn reads_image_dump_files() {
        let Some(image) = std::env::var_os("WAL_TEST_IMAGE") else { return };
        let files = dump_files(Path::new(&image)).unwrap();
        eprintln!("{files:?}");
        assert!(!files.is_empty());
    }
}
