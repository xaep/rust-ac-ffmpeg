use std::ptr;

use std::ffi::{CStr, CString};

use libc::{c_char, c_int, c_uint, c_void};

use crate::Error;

use crate::codec::CodecParameters;
use crate::format::io::Writer;
use crate::packet::Packet;

extern "C" {
    fn ffw_guess_output_format(
        short_name: *const c_char,
        file_name: *const c_char,
        mime_type: *const c_char,
    ) -> *mut c_void;

    fn av_free(ptr: *mut c_void);

    fn ffw_muxer_new() -> *mut c_void;
    fn ffw_muxer_get_nb_streams(muxer: *const c_void) -> c_uint;
    fn ffw_muxer_new_stream(muxer: *mut c_void, params: *const c_void) -> c_int;
    fn ffw_muxer_init(
        muxer: *mut c_void,
        io_context: *mut c_void,
        format: *mut c_void,
        options: *mut c_void,
    ) -> c_int;
    fn ffw_muxer_get_option(muxer: *mut c_void, key: *const c_char, out: *mut *mut c_char)
        -> c_int;
    fn ffw_muxer_set_option(muxer: *mut c_void, key: *const c_char, value: *const c_char) -> c_int;
    fn ffw_muxer_write_frame(muxer: *mut c_void, packet: *mut c_void) -> c_int;
    fn ffw_muxer_interleaved_write_frame(muxer: *mut c_void, packet: *mut c_void) -> c_int;
    fn ffw_muxer_free(muxer: *mut c_void);
}

/// Muxer builder.
pub struct MuxerBuilder {
    ptr: *mut c_void,
    interleaved: bool,
}

impl MuxerBuilder {
    /// Create a new muxer builder.
    fn new() -> MuxerBuilder {
        let ptr = unsafe { ffw_muxer_new() };

        if ptr.is_null() {
            panic!("unable to allocate a muxer context");
        }

        MuxerBuilder {
            ptr: ptr,
            interleaved: false,
        }
    }

    /// Add a new stream with given parameters.
    pub fn add_stream(&mut self, params: &CodecParameters) -> Result<(), Error> {
        let res = unsafe { ffw_muxer_new_stream(self.ptr, params.as_ptr()) };

        if res < 0 {
            return Err(Error::new("unable to create a new stream"));
        }

        Ok(())
    }

    /// Set the muxer to do the interleaving automatically. It is disabled by
    /// default.
    pub fn interleaved(mut self, interleaved: bool) -> MuxerBuilder {
        self.interleaved = interleaved;
        self
    }

    /// Build the muxer.
    ///
    /// # Arguments
    /// * `io_context` - an AVIO writer
    /// * `format` - an output format
    pub fn build<T>(mut self, mut io_context: T, format: OutputFormat) -> Result<Muxer<T>, Error>
    where
        T: Writer,
    {
        let io_context_ptr = io_context.as_mut_ptr();
        let format_ptr = format.ptr;

        let res = unsafe { ffw_muxer_init(self.ptr, io_context_ptr, format_ptr, ptr::null_mut()) };

        if res < 0 {
            return Err(Error::new("unable to initialize the muxer"));
        }

        let muxer_ptr = self.ptr;

        self.ptr = ptr::null_mut();

        let res = Muxer {
            ptr: muxer_ptr,
            io_context: io_context,
            interleaved: self.interleaved,
        };

        Ok(res)
    }
}

impl Drop for MuxerBuilder {
    fn drop(&mut self) {
        unsafe { ffw_muxer_free(self.ptr) }
    }
}

unsafe impl Send for MuxerBuilder {}
unsafe impl Sync for MuxerBuilder {}

/// Muxer.
pub struct Muxer<T> {
    ptr: *mut c_void,
    io_context: T,
    interleaved: bool,
}

impl Muxer<()> {
    /// Get a muxer builder.
    pub fn builder() -> MuxerBuilder {
        MuxerBuilder::new()
    }
}

impl<T> Muxer<T> {
    /// Get option.
    pub fn get_option(&self, name: &str) -> Option<String> {
        let name = CString::new(name).expect("invalid option name");

        let mut value = ptr::null_mut() as *mut c_char;

        let value_ptr = &mut value as *mut *mut c_char;

        unsafe {
            let ret = ffw_muxer_get_option(self.ptr, name.as_ptr() as _, value_ptr);

            if ret < 0 {
                if !value.is_null() {
                    av_free(value as _);
                }

                panic!("invalid option");
            } else if value.is_null() {
                None
            } else {
                let v = CStr::from_ptr(value as _)
                    .to_str()
                    .expect("option is not UTF-8 encoded")
                    .to_string();

                av_free(value as _);

                Some(v)
            }
        }
    }

    /// Set an option.
    pub fn set_option<V>(&mut self, name: &str, value: V)
    where
        V: ToString,
    {
        let name = CString::new(name).expect("invalid option name");
        let value = CString::new(value.to_string()).expect("invalid option value");

        let ret =
            unsafe { ffw_muxer_set_option(self.ptr, name.as_ptr() as _, value.as_ptr() as _) };

        if ret < 0 {
            panic!("invalid option");
        }
    }

    /// Mux a given packet. The packet pts and dts are expected to be in
    /// microseconds. They will be automatically rescaled to match the time
    /// base of the corresponding stream.
    pub fn push(&mut self, mut packet: Packet) -> Result<(), Error> {
        let nb_streams = unsafe { ffw_muxer_get_nb_streams(self.ptr) as usize };

        assert!(packet.stream_index() < nb_streams);

        let res = unsafe {
            if self.interleaved {
                ffw_muxer_interleaved_write_frame(self.ptr, packet.as_mut_ptr())
            } else {
                ffw_muxer_write_frame(self.ptr, packet.as_mut_ptr())
            }
        };

        if res < 0 {
            Err(Error::new("unable to write a given packet"))
        } else {
            Ok(())
        }
    }

    /// Flush the muxer.
    pub fn flush(&mut self) -> Result<(), Error> {
        let res = unsafe {
            if self.interleaved {
                ffw_muxer_interleaved_write_frame(self.ptr, ptr::null_mut())
            } else {
                ffw_muxer_write_frame(self.ptr, ptr::null_mut())
            }
        };

        if res < 0 {
            Err(Error::new("unable to flush the muxer"))
        } else {
            Ok(())
        }
    }

    /// Get reference to the underlying IO.
    pub fn io(&self) -> &T {
        &self.io_context
    }

    /// Get mutable reference to the underlying IO.
    pub fn io_mut(&mut self) -> &mut T {
        &mut self.io_context
    }
}

impl<T> Drop for Muxer<T> {
    fn drop(&mut self) {
        unsafe { ffw_muxer_free(self.ptr) }
    }
}

unsafe impl<T> Send for Muxer<T> where T: Send {}
unsafe impl<T> Sync for Muxer<T> where T: Sync {}

/// FFmpeg output format.
pub struct OutputFormat {
    ptr: *mut c_void,
}

impl OutputFormat {
    /// Try to find an output format by its name.
    pub fn find_by_name(name: &str) -> Option<OutputFormat> {
        let name = CString::new(name).expect("invalid format name");

        let ptr =
            unsafe { ffw_guess_output_format(name.as_ptr() as *const _, ptr::null(), ptr::null()) };

        if ptr.is_null() {
            return None;
        }

        let res = OutputFormat { ptr: ptr };

        Some(res)
    }

    /// Try to find an output format by the MIME type.
    pub fn find_by_mime_type(mime_type: &str) -> Option<OutputFormat> {
        let mime_type = CString::new(mime_type).expect("invalid MIME type");

        let ptr = unsafe {
            ffw_guess_output_format(ptr::null(), ptr::null(), mime_type.as_ptr() as *const _)
        };

        if ptr.is_null() {
            return None;
        }

        let res = OutputFormat { ptr: ptr };

        Some(res)
    }

    /// Try to guess an output format from a file name.
    pub fn guess_from_file_name(file_name: &str) -> Option<OutputFormat> {
        let file_name = CString::new(file_name).expect("invalid file name");

        let ptr = unsafe {
            ffw_guess_output_format(ptr::null(), file_name.as_ptr() as *const _, ptr::null())
        };

        if ptr.is_null() {
            return None;
        }

        let res = OutputFormat { ptr: ptr };

        Some(res)
    }
}

unsafe impl Send for OutputFormat {}
unsafe impl Sync for OutputFormat {}
