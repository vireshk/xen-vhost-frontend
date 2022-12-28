// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0
//
// This file keeps list of the supported devices and their designated Virtio device ids.

use lazy_static::lazy_static;

lazy_static! {
    pub static ref SUPPORTED_DEVICES: Vec<(&'static str, u32)> = vec![("i2c", 22), ("gpio", 29)];
}
