# msgs fixtures

The agent-inventory UDP messages the viewer handles (`UpdateCreateInventoryItem`,
`BulkUpdateInventory` and the others), decoded into model updates and callbacks.

Filled by recorder R4 of the inventory rung (vkstorm-rust rung seven). The
fixtures are recorded from the unmodified viewer C++, before any port commit,
on Windows and Linux, and are never edited by hand. To change them, change the
recorder and record them again.
