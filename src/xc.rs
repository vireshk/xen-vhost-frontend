use std::mem;
use std::ptr;

use libxen_sys::*;
use super::{Error, Result};

pub struct XenCtrl {
    xci: *mut xc_interface,
}

impl XenCtrl {
    pub fn new() -> Result<Self> {
        let xci = unsafe {
            xc_interface_open(ptr::null_mut::<xentoollog_logger>(), ptr::null_mut::<xentoollog_logger>(), 0)
        };

        if xci.is_null() {
            return Err(Error::XsError);
        }

        Ok (Self {
            xci,
        })
    }

    pub fn get_dom_mem(&self, domid: domid_t) -> Result<u64> {
        let mut info: xc_dominfo = unsafe { mem::zeroed() };

        let ret = unsafe {
            xc_domain_getinfo(self.xci, domid as u32, 1, &mut info)
        };

        if ret != 1 || info.domid != domid as u32 {
            Err(Error::XsError)
        } else {
            Ok((info.nr_pages - 4) << XC_PAGE_SHIFT)
        }
    }
}

impl Drop for XenCtrl {
    fn drop(&mut self) {
        unsafe{ xc_interface_close(self.xci); }
    }
}

