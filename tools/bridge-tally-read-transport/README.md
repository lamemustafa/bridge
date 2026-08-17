# Bridge Tally read transport

This crate is the only network boundary used by the live compatibility reader.
Its public API accepts a sealed `ReadOnlyProfile`; it cannot accept arbitrary
XML, imports, or write requests. The lower-level generic HTTP transport remains
private behind this adapter.
