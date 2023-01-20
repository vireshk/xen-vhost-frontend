# `xen-vhost-frontend`

## Description
This program implements Xen specific `vhost-user-frontend`. This is PoC
(proof-of-concept) implementation which lets us verify hypervisor-agnosticism of
Rust based `vhost-user` backends.

This is only tested for `AARCH64` until now.

## Key components

- [xen-vhost-frontend](https://github.com/vireshk/xen-vhost-frontend/tree/main)

  This is the Xen specific implementation of the `vhost-user` protocol, i.e. the
  current crate. This is designed based on the EPAM's
  [virtio-disk](https://github.com/xen-troops/virtio-disk) implementation.

- [Xen](https://github.com/xen-project/xen)

  Enable following config options:

  ```
  diff --git a/xen/arch/arm/configs/arm64_defconfig b/xen/arch/arm/configs/arm64_defconfig
  index e69de29bb2d1..38ca05a8b416 100644
  --- a/xen/arch/arm/configs/arm64_defconfig
  +++ b/xen/arch/arm/configs/arm64_defconfig
  @@ -0,0 +1,3 @@
  +CONFIG_IOREQ_SERVER=y
  +CONFIG_EXPERT=y

  ```

  Latest tested HEAD:

  ```
  commit c8aaebccc8e8 ("tools/libxl: Fix virtio build error for 32-bit platforms")
  ```

- [vhost-device](https://github.com/rust-vmm/vhost-device/tree/main)

  These are Rust based `vhost-user` backends, maintained inside the rust-vmm
  project. These are not required to be modified based on hypervisor and are
  truly hypervisor-agnostic.

- [Linux Kernel](https://git.kernel.org/pub/scm/linux/kernel/git/vireshk/linux.git/log/?h=xen/host-guest)

  The current setup doesn't work with Vanilla kernel and needs some changes
  (hacks). This must be used for the Dom0 kernel. The same image can be used for
  guests too, but it is not mandatory.


## Test Setup

The following steps lets one test I2C `vhost-device` on Xen.

- Build Xen for aarch64:

  ```
  $ ./configure --libdir=/usr/lib --build=x86_64-unknown-linux-gnu --host=aarch64-linux-gnu \
    --disable-docs --disable-golang --disable-ocamltools \
    --with-system-qemu=/root/qemu/build/i386-softmmu/qemu-system-i386
  
  $ make -j9 debball CROSS_COMPILE=aarch64-linux-gnu- XEN_TARGET_ARCH=arm64
  ```

- Run Xen via Qemu on X86 machine:

  ```
  $ qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a57 -serial mon:stdio \
    -device virtio-net-pci,netdev=net0 -netdev user,id=net0,hostfwd=tcp::8022-:22 \
    -drive file=/home/debian-bullseye-arm64.qcow2,index=0,id=hd0,if=none,format=qcow2 \
    -device virtio-scsi-pci -device scsi-hd,drive=hd0 \
    -display none -m 8192 -smp 8 -kernel /home/xen/xen \
    -append "dom0_mem=5G,max:5G dom0_max_vcpus=7 loglvl=all guest_loglvl=all" \
    -device guest-loader,addr=0x46000000,kernel=/home/Image,bootargs="root=/dev/sda2 console=hvc0 earlyprintk=xen" \
    -device ds1338,address=0x20
  ```
  The `ds1338` entry here is required to create a virtual I2C based RTC device
  on Dom0.

  This should get Dom0 up and running.

- Build `xen-vhost-frontend` crate:

  ```
  $ git clone https://github.com/vireshk/xen-vhost-frontend
  $ cd xen-vhost-frontend
  $ cargo build --release
  $ cd ../
  ```

- Build `vhost-device` crate:

  ```
  $ git clone https://github.com/rust-vmm/vhost-device
  $ cd vhost-device
  $ cargo build --release
  $ cd ../
  ```

- Setup I2C based RTC devices on Dom0

  This is required to control the device on Dom0 from the guest.

  ```
  $ echo ds1338 0x20 > /sys/bus/i2c/devices/i2c-0/new_device
  $ echo 0-0020 > /sys/bus/i2c/devices/0-0020/driver/unbind
  ```

- Lets run everything

  First start the I2C backend. This tells the I2C backend to hook up to
  `/root/i2c.sock0` socket and wait for the master to start transacting. The
  I2C controller used here on Dom0 is named `90c0000.i2c` (can be read from
  `/sys/bus/i2c/devices/i2c-0/name`) and `32` here matches the device on I2C bus
  set in the previous commands (`0x20`).

  ```
  $ /root/vhost-device/target/release/vhost-device-i2c -s /root/i2c.sock -c 1 -l 90c0000.i2c:32'
  ```

  Then start xen-vhost-frontend. This provides the path of the socket to the
  master side.

  ```
  $ /root/xen-vhost-frontend/target/release/xen-vhost-frontend --socket-path /root/'
  ```

  It supports I2C and GPIO for now. You can add support for more devices by
  adding a relevant entry in `src/supported_devices.rs` file.

  Now that all the preparations are done, lets start the guest. The guest kernel
  should have Virtio related config options enabled, along with `i2c-virtio`
  driver.

  ```
  $ xl create -c domu.conf
  ```

  The guest should boot now. Once the guest is up, you can create the I2C based
  RTC device and use it. Following will create `/dev/rtc0` in the guest, which you
  can configure with the standard `hwclock` utility.

  ```
  $ echo ds1338 0x20 > /sys/bus/i2c/devices/i2c-0/new_device
  ```

## Sample domu.conf

  ```
  kernel="/root/Image"
  memory=512
  vcpus=3
  command="console=hvc0 earlycon=xenboot"
  name="domu"
  virtio = [ "type=virtio,device22, transport=mmio" ]
  ```

  The device type here defines the device to be emulated on the guest. The type
  value is set with the DT `compatible` string of the device. For example,
  it is `virtio,device22` for I2C and `virtio,device29` for GPIO.
