# Captured macOS SIGTRAP control

Captured at 2026-09-08T13:41:56.277223+00:00 from a disposable `bridge_lib-cafe` process on macOS. The process was compiled from `#include <signal.h>` and `int main(void) { return raise(SIGTRAP); }` and exited with signal 5. This is a collector control, not the unexplained application-test failure.

Raw report SHA-256: `e74da8d2157556ec96304d1def818f860c3d5130207ebaff4d678e7faeed0407`. The raw report is not committed because it contains process/user metadata. This privacy-reduced IPS preserves the captured two-object format, `bug_type`, process name, exception type/signal, faulting-thread index, thread trigger flags, frame/image indexes, symbols and relative offsets, and image basenames. Other fields, including paths, identifiers, registers and absolute memory addresses, were omitted; retained stack values were not invented.

The [hosted calibration workflow](../../.github/workflows/macos-crash-capture-control.yml) reproduces the same owned control operation and verifies capture on a hosted Mac. [Its initial successful run](https://github.com/lamemustafa/bridge/actions/runs/34237062493) corroborates the capture mechanism; this fixture originated from the local control report above. The tests add synthetic privacy markers and bounded-shape mutations to this captured structure.

Format reference: [Apple IPS crash reports](https://developer.apple.com/documentation/xcode/interpreting-the-json-format-of-a-crash-report).
