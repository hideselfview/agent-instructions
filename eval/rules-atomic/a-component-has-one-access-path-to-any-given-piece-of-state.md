## A component has one access path to any given piece of state

Don't both take a piece of state as a prop and read the same state from global
context/environment. That's duplication: it makes the component's role ambiguous
(pure prop-driven leaf vs context-aware container) and forces tests/previews to
provide both paths.

Framework instances of the same anti-pattern:

- **SwiftUI**: `@Environment(UiState.self)` + `@Binding var foo` to a field on
  `uiState`.
- **React**: `useContext(StoreContext)` + `props.foo` for the same data.
- **Dioxus**: `use_context::<Store>()` + a signal prop pointing at the same
  value.

Default fix: remove the context/environment read (make the component fully
prop-driven). Going the other way — fully context-driven — usually wrecks
preview/test ergonomics because the fixture has to construct a real context
value.

Legitimate reasons to take a prop/binding when context is also available:

- A framework API demands it (`NavigationStack.path`, `Picker.selection`,
  controlled form inputs, refs).
- State lives in a parent's local state and isn't in the global context — the
  prop is the only path.
- The component is intentionally pure / prop-driven — doesn't touch context at
  all.
