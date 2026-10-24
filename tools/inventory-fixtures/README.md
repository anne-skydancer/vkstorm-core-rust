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

Fixtures are recorded from the viewer's C++ as it is on the recording
commit, never edited by hand, and replayed on Windows and Linux. That C++
includes the inventory fixes approved from the oracles' findings (the F1
and AIS fixes of #174, and the legacy-text, model and task-inventory fixes
of #181); each recorder's README names the commit it recorded and the
fixes that changed its records. Behaviour still recorded as it is, defects
included, is listed in each README's findings.
