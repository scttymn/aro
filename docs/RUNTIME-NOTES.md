# Runtime notes (what Android needs from the host, as discovered)

Facts established on the development machine (kernel 7.1, Omarchy) while
reaching M1. Each one is something `aro-exec` now does.

## Namespaces and root

- An unprivileged user namespace (`unshare(CLONE_NEWUSER|NEWNS|NEWPID|NEWIPC|NEWUTS)`)
  is enough. The host uid maps to 0 inside; bionic checks that
  `/dev/__properties__` files are owned by uid 0, so the mapping must be to 0.
- `unshare(CLONE_NEWUSER)` needs a single-threaded process, and no thread can be
  created after `unshare(CLONE_NEWPID)`. The parent therefore runs an event loop
  (poll on the log socket + `waitpid(WNOHANG)`), not threads.
- Read uid/gid before `unshare`; inside the new namespace they read as the
  overflow id and the id map write fails with EPERM.
- Root is a tmpfs with `/system` and `/apex` bind-mounted read-only from the
  unpacked image, `/data` read-write, `/bin` and `/etc` symlinks into `/system`,
  a fresh `/dev` tmpfs with `null zero full random urandom tty` bound from the
  host, `/proc` mounted fresh (works in the new PID namespace), `/sys` bound
  read-only, then `pivot_root`.

## Binder

- binderfs mounts inside an unprivileged user namespace (`mount -t binder`),
  giving a private binder instance per IPC namespace. Devices are created with
  `BINDER_CTL_ADD` on `binder-control`. Protocol version 8 on this kernel's Rust
  binder driver. `/dev/binder` is a symlink to `/dev/binderfs/binder`.
- `app_process64` opens `/dev/binder` during `RuntimeInit` and aborts if it is
  missing. It does not need a service manager to reach the app's `main`; any
  `ServiceManager.getService` call blocks and retries until one exists (M2).

## APEX modules

- The GSI ships ART, runtime, i18n, conscrypt, sdkext and others as APEX
  archives in `/system/apex`; `.capex` wraps an `.apex` in `original_apex`. The
  payload is an ext4 image, extracted to `/apex/<name>`.
- `linkerconfig` and `derive_classpath` learn the active modules from
  `/apex/apex-info-list.xml`, which apexd normally writes. `aro-exec` writes it
  from each module's `apex_manifest.pb` (name, version).

## Linker

- bionic's linker requires `/linkerconfig/ld.config.txt`; without it, APEX
  libraries are not found and `dalvikvm64` fails to link `libnativehelper.so`.
  `/apex/com.android.runtime/bin/linkerconfig --target /linkerconfig` generates it
  inside the namespace (it only reads the image and the apex list).

## Properties

- bionic reads `/dev/__properties__`: `property_info` (name to context trie),
  `properties_serial`, and one 128 KiB property area per context. `aro-props`
  writes these (single context `u:object_r:default_prop:s0`). Verified with
  Android's `getprop` and `derive_classpath`.
- Beyond the GSI's `build.prop` the image expects vendor-provided values:
  `ro.product.cpu.abilist{,32,64}`, `ro.dalvik.vm.native.bridge`,
  `dalvik.vm.heap*`. Missing ones make `RuntimeInit` abort.

## ART

- `ro.dalvik.vm.enable_uffd_gc=true` selects the CMC collector; userfaultfd in
  user-mode-only form works unprivileged (`vm.unprivileged_userfaultfd=0` on the
  host). Without it ART picks CC and rejects the prebuilt boot image (read
  barrier mismatch).
- The GSI's prebuilt boot image is still rejected ("component count 38,
  expected <= 35" for `boot-framework-adservices.art`), so ART runs imageless.
  Cost: about 0.1 s to load an APK class bare, about 1.1 s through
  `app_process64`. Fixing this (own boot image via `dex2oat`, or matching the
  image's classpath) is an optimisation for later.
- Classpath: `derive_classpath` produces `BOOTCLASSPATH` (50 jars),
  `DEX2OATBOOTCLASSPATH`, `SYSTEMSERVERCLASSPATH`; `aro-exec` runs it once and
  caches `/data/local/tmp/classpath.env`.
- `/data/dalvik-cache/x86_64` must exist or ART warns for every dex.

## Logging

- Android logs go to `/dev/socket/logdw` datagrams, never to stderr. `aro-exec`
  binds that socket and prints `PRI/tag(tid): message`. Crash reports from
  `debuggerd` arrive the same way, with symbolised backtraces.
- aconfig flag reads want `/metadata/aconfig/{maps,boot}`; laid out from
  `/system/etc/aconfig` and each APEX's `etc/` (29 containers).

## Image source

- Android 17 QPR2 beta GSI `aosp_x86_64-exp-CP41.260814.003.B1` (SDK 37, built
  2026-08-25), raw ext4 `system.img` inside the zip. Development-only license;
  ship from an AOSP source build.
