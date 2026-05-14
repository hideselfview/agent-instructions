---
paths:
  - "**/*.swift"
  - "**/*.tsx"
  - "**/*.jsx"
  - "bae-web/**/*.rs"
---

# Reactive UI rules

## Read at the leaf, not the parent

Pass the observable object (store, app service) down. Let the leaf
component read the specific field it needs. Don't read in a parent and
pass the value — that subscribes the parent to changes it doesn't care
about. The reactive framework tracks which view accessed which property;
reading in a parent widens the re-render scope unnecessarily.

## A component has one access path to any given piece of state

Don't both take a piece of state as a prop and read the same state from
global context/environment. That's duplication: it makes the component's
role ambiguous (pure prop-driven leaf vs context-aware container) and
forces tests/previews to provide both paths.

Framework instances of the same anti-pattern:

- **SwiftUI**: `@Environment(UiState.self)` + `@Binding var foo` to a
  field on `uiState`.
- **React**: `useContext(StoreContext)` + `props.foo` for the same data.
- **Dioxus**: `use_context::<Store>()` + a signal prop pointing at the
  same value.

Default fix: remove the context/environment read (make the component
fully prop-driven). Going the other way — fully context-driven — usually
wrecks preview/test ergonomics because the fixture has to construct a
real context value.

Legitimate reasons to take a prop/binding when context is also available:

- A framework API demands it (`NavigationStack.path`, `Picker.selection`,
  controlled form inputs, refs).
- State lives in a parent's local state and isn't in the global context
  — the prop is the only path.
- The component is intentionally pure / prop-driven — doesn't touch
  context at all.

## State describes what is, not what should happen

Don't use observable state fields as imperative command triggers. The
"set a flag, observe via `.task(id:)`/`onChange`, reset the flag"
pattern conflates two concepts: state is what the UI *is*; commands are
what should *happen once*. Packing a one-shot command into an observable
field leaks it through every reader, requires edge-triggered guards,
and creates subtle bugs (equal-value sets don't re-fire).

For ephemeral commands (scroll, flash, focus, play-this-once), use
pub/sub:

- **SwiftUI**: `PassthroughSubject` / `AsyncStream` — sender publishes,
  receiver subscribes via `.onReceive`.
- **React**: event emitter or imperative ref method.
- **Dioxus**: channel or explicit signal dispatch.

Diagnostic: if you find yourself writing `uiState.xyz = value` followed
by a reader that resets `uiState.xyz = nil` after handling, the design
is wrong — replace with a subject.

## Pub/sub over ref-registration

When component A needs to push imperative updates to component B —
bypassing the normal reactive flow — prefer pub/sub: B subscribes to a
stream A publishes to. Avoid ref-registration, where intermediate
containers collect refs to B and the parent calls methods on them.

Registration forces every layer in the hierarchy to know B's type and
wire the connection, leaking B through containers that shouldn't care
about it. Pub/sub lets B self-wire at construction — no registration
step, no leaked types, no coupling through the tree.

Framework instances:

- **SwiftUI**: `PassthroughSubject` / `AsyncStream` — A publishes, B
  subscribes via `.onReceive` or a `.task` await loop.
- **React**: event emitter (`EventTarget`, mitt, RxJS subject) or a
  context with a callback registry.
- **Dioxus**: channel (`futures::channel::mpsc`) or a signal B watches.

Fall back to registration only when the update requires calling
multiple methods on a stateful object that can't be captured in a
single message.

## Reducers must not read state to write state

Event handlers that mutate state must derive their output purely from
the event payload. If a reducer reaches into other state slices
(`summaries[albumId]`) to compute what it writes
(`storageSummaries[releaseId]`), the store becomes ordering-dependent
— whichever slice populated first wins — and forces fallback defaults
on cache miss.

Fix: widen the event payload so the reducer is a pure function of the
event. Events can carry foreign-key ids, but must not require the
reducer to dereference them against other slices. On the producer side
(emitter, bridge, backend), this may mean joining data before emitting
so the event payload mirrors what every reducer consumer needs — not
the raw DB row.

## UI iterates and renders, nothing more

The UI's job is to iterate and render. Domain logic — sorting,
filtering, grouping, computing, formatting, deriving — belongs in the
data layer (core, store, server). The state layer should deliver data
in the exact shape the UI needs: pre-sorted, pre-grouped, pre-formatted,
with pre-computed flags. The UI maps that structure to visual elements.

Exceptions:

- Type coercion for rendering (e.g., `Int → String` for `Text()`,
  `String → URL` for image loading).
- Localized string building and matching (platform-native localization
  APIs).

## Pass reactive handles, not snapshots

When a component needs to react to changes in a value, accept the
reactive handle as a prop — not a one-time snapshot.

- **Dioxus**: take `ReadSignal<T>` for read-only reactive props; take
  `Signal<T>` (or a specific write handle) when the component needs to
  update. Plain `T` is a one-time value — the component won't re-render
  when the parent's underlying state changes.
- **SwiftUI**: take `@Binding<T>` for two-way reactivity (child reads
  and writes parent state). Take `@Bindable var model: Model` when the
  child needs to bind into fields of an `@Observable`. For one-way
  read, "Read at the leaf, not the parent" (above) covers it — pass
  the `@Observable` down, read the field at the leaf.
