# Bridge Tally portable runtime

This crate owns the read-side endpoint execution control plane independently of
Tauri, SQLCipher, and the native application. It provides per-endpoint
serialization, queue deadlines, cancellation, request spacing, circuit
admission, deterministic bounded transient-read retry, and fixed-cardinality
privacy-reduced observations.

The public operation enum contains no import or write variant. The generic
closure is an integration seam rather than proof about the closure's behavior;
native dependency and source checks must continue to ensure that write paths do
not call the read retry API.
