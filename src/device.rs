// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use seccompiler::SeccompAction;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use lazy_static::lazy_static;
use vhost_user_frontend::{Generic, VhostUserConfig, VirtioDeviceType};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::{
    interrupt::XenInterrupt, mmio::XenMmio, supported_devices::SUPPORTED_DEVICES,
    Error, Result,
};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct DeviceArgs {
    /// Location of vhost-user Unix domain socket.
    #[clap(short = 's', long)]
    socket_path: String,
    /// Device name for loopback test.
    #[clap(short = 'd', long, default_value = "i2c")]
    device: String,
}

#[derive(Debug)]
struct DeviceInfo {
    name: &'static str,
    compatible: String,
    index: u32,
}

impl DeviceInfo {
    fn new(name: &'static str, id: u32) -> Self {
        DeviceInfo {
            name,
            compatible: format!("virtio,device{}", id),
            index: 0,
        }
    }

    fn index(&mut self) -> String {
        self.index += 1;
        (self.index - 1).to_string()
    }
}

lazy_static! {
    static ref DEVICES: Mutex<HashMap<String, DeviceInfo>> = {
        let mut map = HashMap::new();

        for entry in SUPPORTED_DEVICES.iter() {
            let dev = DeviceInfo::new(entry.0, entry.1);
            map.insert(dev.compatible.clone(), dev);
        }
        Mutex::new(map)
    };
    static ref DEVICE_ARGS: DeviceArgs = DeviceArgs::parse();
}

pub struct XenDevice {
    pub gdev: Mutex<Generic>,
    pub mmio: Mutex<XenMmio>,
    interrupt: Mutex<Option<Arc<XenInterrupt>>>,
}

impl XenDevice {
    pub fn new() -> Result<Arc<Self>> {
        let val = SUPPORTED_DEVICES
        .iter()
        .find(|(name, _)| *name == DEVICE_ARGS.device).unwrap().1;
        let compat = format!("virtio,device{}", val);

        let mut devices = DEVICES.lock().unwrap();
        let dev = devices
            .get_mut(&compat)
            .ok_or(Error::XenDevNotSupported(compat.to_string()))?;

        let device_type = VirtioDeviceType::from(dev.name);
        let (num, size) = device_type.queue_num_and_size();

        let vu_cfg = VhostUserConfig {
            socket: DEVICE_ARGS.socket_path.to_owned() + dev.name + ".sock" + &dev.index(),
            num_queues: num,
            queue_size: size as u16,
        };

        println!(
            "Connecting to {} device backend over {} socket..",
            dev.name, vu_cfg.socket
        );

        let gdev = Generic::new(
            vu_cfg,
            SeccompAction::Allow,
            EventFd::new(EFD_NONBLOCK).unwrap(),
            device_type,
        )
        .map_err(Error::VhostFrontendError)?;

        let mmio = XenMmio::new(&gdev)?;

        let dev = Arc::new(Self {
            gdev: Mutex::new(gdev),
            mmio: Mutex::new(mmio),
            interrupt: Mutex::new(None),
        });

        // Wait for the host to create the misc device.
        while dev.mmio.lock().unwrap().setup_vmsg_events(dev.clone()).is_err() {}
        *dev.interrupt.lock().unwrap() = Some(XenInterrupt::new(dev.clone(), false));

        Ok(dev)
    }

    pub fn interrupt(&self) -> Arc<XenInterrupt> {
        // We use interrupt.take() here to drop the reference to Arc<XenInterrupt>, as the same
        // isn't required anymore.
        self.interrupt.lock().unwrap().as_ref().unwrap().clone()
    }
}
