//! Drive letter redirection for path arguments of hooked file APIs.

/// A path argument, rewritten when it points to the redirected drive.
pub struct Redirected<T> {
    original: *const T,
    replaced: Option<Vec<T>>,
}

impl<T> Redirected<T> {
    /// Pointer to pass to the original API (valid while `self` lives).
    pub fn ptr(&self) -> *const T {
        self.replaced.as_ref().map_or(self.original, |v| v.as_ptr())
    }
}

/// Rewrites `X:\rest` (or `X:/rest`, `X:`) to `target\rest`, for narrow (u8) or wide
/// (u16) NUL terminated strings. Other paths, including drive-relative `X:file`, are kept.
///
/// # Safety
/// `path` must be null or a valid NUL terminated string.
pub unsafe fn rewrite_drive<T>(path: *const T, drive: u8, target: &[T]) -> Redirected<T>
where
    T: Copy + Into<u32> + From<u8>,
{
    let mut out = Redirected { original: path, replaced: None };
    if path.is_null() {
        return out;
    }
    let mut len = 0;
    while unsafe { (*path.add(len)).into() } != 0 {
        len += 1;
    }
    let s = unsafe { std::slice::from_raw_parts(path, len) };
    let c = |i: usize| s.get(i).map_or(0, |v| (*v).into());
    let on_drive = c(0) == drive.to_ascii_uppercase() as u32 || c(0) == drive.to_ascii_lowercase() as u32;
    if !on_drive || c(1) != b':' as u32 || !matches!(c(2), 0 | 0x5C | 0x2F) {
        return out;
    }
    let mut new: Vec<T> = target.to_vec();
    if len > 3 {
        new.push(T::from(b'\\'));
        new.extend_from_slice(&s[3..]);
    }
    new.push(T::from(0));
    out.replaced = Some(new);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(path: &str) -> String {
        let p: Vec<u8> = path.bytes().chain([0]).collect();
        let r = unsafe { rewrite_drive(p.as_ptr(), b'D', b"C:\\game\\WindowsLoader") };
        let out = unsafe { std::ffi::CStr::from_ptr(r.ptr().cast()) };
        out.to_string_lossy().into_owned()
    }

    #[test]
    fn redirects_drive() {
        assert_eq!(run("D:\\AH21\\save.bin"), "C:\\game\\WindowsLoader\\AH21\\save.bin");
        assert_eq!(run("d:/x"), "C:\\game\\WindowsLoader\\x");
        assert_eq!(run("D:\\"), "C:\\game\\WindowsLoader");
        assert_eq!(run("D:"), "C:\\game\\WindowsLoader");
        assert_eq!(run("C:\\D:\\x"), "C:\\D:\\x");
        assert_eq!(run("Data.bin"), "Data.bin");
        assert_eq!(run("D:file"), "D:file");
    }

    #[test]
    fn wide() {
        let p: Vec<u16> = "D:\\a".encode_utf16().chain([0]).collect();
        let t: Vec<u16> = "E:\\t".encode_utf16().collect();
        let r = unsafe { rewrite_drive(p.as_ptr(), b'D', &t) };
        let out = unsafe { std::slice::from_raw_parts(r.ptr(), 7) };
        assert_eq!(String::from_utf16_lossy(out), "E:\\t\\a\0");
    }
}
