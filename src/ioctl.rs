// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use libxen_sys::{domid_t, XC_PAGE_SHIFT};
use xen_ioctls::domctl::{domctl::xc_domain_info};

use super::{Error, Result};

pub fn get_dom_mem(domid: domid_t) -> Result<u64> {
    let info = xc_domain_info(domid, 1);

    if info.len() != 1 || info[0].domid != domid {
        Err(Error::XsError)
    } else {
        Ok((info[0].nr_pages - 4) << XC_PAGE_SHIFT)
    }
}
