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
use vhost_user_frontend::{Generic, VhostUserConfig, VirtioDevice, VirtioDeviceType};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};
use xen_bindings::bindings::ioreq;

use super::{
    guest::XenGuest, interrupt::XenInterrupt, mmio::XenMmio, supported_devices::SUPPORTED_DEVICES,
    Error, Result, XsHandle, BACKEND_PATH,
};

pub const VIRTIO_MMIO_IO_SIZE: u64 = 0x200;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct DeviceArgs {
    /// Location of vhost-user Unix domain socket.
    #[clap(short, long)]
    socket_path: String,
    /// Memory mapping, foreign or grant.
    #[clap(short, long)]
    foreign_mapping: bool,
}

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
    pub xsh: XsHandle,
    pub dev_id: u32,
    pub addr: u64,
    pub irq: u8,
    pub guest: Arc<XenGuest>,
    interrupt: Mutex<Option<Arc<XenInterrupt>>>,
}

impl XenDevice {
    pub fn new(dev_id: u32, guest: Arc<XenGuest>) -> Result<Arc<Self>> {
        let mut xsh = XsHandle::new()?;
        let be = xsh.connect_dom(dev_id, guest.fe_domid)?;

        let dev_dir = format!("{}/{}/{}", BACKEND_PATH, guest.fe_domid, dev_id);
        let compatible = xsh.read_str(&dev_dir, "type")?;
        let addr = xsh.read_int(&be, "base")? as u64;
        let irq = xsh.read_int(&be, "irq")? as u8;

        let mut devices = DEVICES.lock().unwrap();
        let dev = devices
            .get_mut(&compatible)
            .ok_or(Error::XenDevNotSupported(compatible))?;

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

        let mmio = XenMmio::new(&gdev, guest.clone(), addr, DEVICE_ARGS.foreign_mapping)?;

        let dev = Arc::new(Self {
            gdev: Mutex::new(gdev),
            mmio: Mutex::new(mmio),
            xsh,
            dev_id,
            addr,
            irq,
            guest,
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

    pub fn setup_ioreq(&self) -> Result<()> {
        self.guest
            .xdm
            .lock()
            .unwrap()
            .map_io_range_to_ioreq_server(self.addr, VIRTIO_MMIO_IO_SIZE)
    }

    pub fn destroy_ioreq(&self) -> Result<()> {
        self.guest
            .xdm
            .lock()
            .unwrap()
            .ummap_io_range_from_ioreq_server(self.addr, VIRTIO_MMIO_IO_SIZE)
    }

    pub fn io_event(&self, ioreq: &mut ioreq) -> Result<()> {
        self.mmio.lock().unwrap().io_event(ioreq, self)
    }

    pub fn exit(&self) {
        if let Some(interrupt) = self.interrupt.lock().unwrap().take() {
            interrupt.exit();
        }

        self.gdev.lock().unwrap().reset();
        self.gdev.lock().unwrap().shutdown();

        self.destroy_ioreq().ok();
    }
}
