use std::io::{self, BufRead, Write};
use std::os::fd::AsRawFd;

pub fn read_secret() -> io::Result<String> {
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: fd is stdin, and original points to writable termios storage.
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr initialized original on success.
    let original = unsafe { original.assume_init() };
    let mut hidden = original;
    hidden.c_lflag &= !libc::ECHO;
    // SAFETY: hidden is initialized termios for the same terminal descriptor.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
        return Err(io::Error::last_os_error());
    }
    struct Restore {
        fd: i32,
        original: libc::termios,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: original was read from this terminal descriptor.
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
            }
        }
    }
    let _restore = Restore { fd, original };
    eprint!("API key: ");
    io::stderr().flush()?;
    let mut key = String::new();
    stdin.lock().read_line(&mut key)?;
    eprintln!();
    Ok(key.trim_end_matches(['\r', '\n']).to_string())
}
