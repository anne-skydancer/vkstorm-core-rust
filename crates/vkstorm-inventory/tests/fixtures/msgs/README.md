# msgs fixtures

The agent-inventory UDP messages the viewer handles (`UpdateCreateInventoryItem`,
`BulkUpdateInventory` and the others), decoded into model updates and callbacks.

Filled by recorder R2 of the inventory rung (vkstorm-rust rung seven), through
the real inventory model, since the handlers live in `llinventorymodel.cpp`.
The fixtures are recorded from the unmodified viewer C++, before any port
commit, on Windows and Linux, and are never edited by hand. To change them,
change the recorder and record them again.
