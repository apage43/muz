//! Coordinator-only VST3 streams. Plugin state never touches the audio callback.
use std::{
    cell::RefCell,
    ffi::c_void,
    io::{Cursor, Read, Seek, SeekFrom, Write},
};
use vst3::{
    Class,
    Steinberg::{IBStream, IBStreamTrait, kInvalidArgument, kResultFalse, kResultOk, tresult},
};
pub(super) struct StateStream(pub RefCell<Cursor<Vec<u8>>>);
impl StateStream {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(RefCell::new(Cursor::new(bytes)))
    }
}
impl Class for StateStream {
    type Interfaces = (IBStream,);
}
impl IBStreamTrait for StateStream {
    unsafe fn read(&self, buffer: *mut c_void, size: i32, read: *mut i32) -> tresult {
        if size < 0 || (size > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        if size == 0 {
            if !read.is_null() {
                unsafe {
                    *read = 0;
                }
            }
            return kResultOk;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(buffer as *mut u8, size as usize) };
        match self.0.borrow_mut().read(out) {
            Ok(n) => {
                if !read.is_null() {
                    unsafe {
                        *read = n as i32;
                    }
                }
                // Steinberg's MemoryStream reports the short count and still returns
                // kResultOk, including at EOF. Plugins may probe the stream this way.
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }
    unsafe fn write(&self, buffer: *mut c_void, size: i32, written: *mut i32) -> tresult {
        if size < 0 || (size > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        let mut stream = self.0.borrow_mut();
        if stream.position().saturating_add(size as u64) > 64 * 1024 * 1024 {
            return kResultFalse;
        }
        let bytes = if size == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) }
        };
        match stream.write(bytes) {
            Ok(n) => {
                if !written.is_null() {
                    unsafe {
                        *written = n as i32;
                    }
                }
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }
    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let how = match mode {
            0 if pos >= 0 => SeekFrom::Start(pos as u64),
            1 => SeekFrom::Current(pos),
            2 => SeekFrom::End(pos),
            _ => return kInvalidArgument,
        };
        match self.0.borrow_mut().seek(how) {
            Ok(p) => {
                if !result.is_null() {
                    unsafe {
                        *result = p as i64;
                    }
                }
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }
    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *pos = self.0.borrow().position() as i64;
        }
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_read_and_eof_report_count_with_success() {
        let stream = StateStream::new(vec![1, 2, 3]);
        let mut out = [0u8; 8];
        let mut read = -1;
        assert_eq!(
            unsafe { stream.read(out.as_mut_ptr().cast(), 8, &mut read) },
            kResultOk
        );
        assert_eq!(read, 3);
        assert_eq!(&out[..3], &[1, 2, 3]);
        assert_eq!(
            unsafe { stream.read(out.as_mut_ptr().cast(), 8, &mut read) },
            kResultOk
        );
        assert_eq!(read, 0);
    }
}
