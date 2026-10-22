# inventory-fixtures

Will hold the drivers that regenerate `crates/vkstorm-inventory/tests/fixtures/`
from the viewer's C++ oracles: the inventory code as it was before
vkstorm-rust rung seven replaced parts of it with `vkstorm-inventory`.

The oracles and their corpora live in the viewer repository, because they
need the viewer's C++ classes; the viewer's test build produces the recording
executables, and the drivers here run them and copy the results into the
crate. Each recorder of the rung owns its own fixture directories:

| recorder | fixtures |
|---|---|
| R1 data model | `model/` |
| R2 inventory model and cache | `cache/` |
| R3 fetch and AIS | `ais/` |
| R4 task inventory, offers, giving, messages | `task/`, `offer/`, `give/`, `msgs/` |

Fixtures are recorded from the unmodified C++, on Windows and Linux, and are
never edited by hand.
