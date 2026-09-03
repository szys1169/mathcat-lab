import Lake
open Lake DSL

package pantograph

lean_lib Pantograph {
  roots := #[`Pantograph]
  defaultFacets := #[LeanLib.sharedFacet]
}

lean_lib Repl {
}

@[default_target]
lean_exe repl {
  root := `Main
  -- Solves the native symbol not found problem
  supportInterpreter := true
}

lean_exe tomograph {
  root := `Tomograph
  -- Solves the native symbol not found problem
  supportInterpreter := true
}

-- The exact upstream commit and archive hash are recorded in
-- PANTOGRAPH_SOURCE_LOCK.json; use the vendored copy for deterministic offline runs.
require LSpec from ".lake/packages/LSpec"
lean_lib PantographTest {
}
@[test_driver]
lean_exe test {
  root := `PantographTest.Main
  -- Solves the native symbol not found problem
  supportInterpreter := true
}
