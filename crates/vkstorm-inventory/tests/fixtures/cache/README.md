# cache fixtures

The inventory model and its cache file (`LLInventoryModel`): cache files
written and read, cache load and reconciliation, mutation sequences and their
outgoing messages, and model validation.

Filled by recorder R2 of the inventory rung (vkstorm-rust rung seven). The
fixtures are recorded from the unmodified viewer C++, before any port commit,
on Windows and Linux, and are never edited by hand. To change them, change the
recorder and record them again.
