## Pass reactive handles, not snapshots

When a component needs to react to changes in a value, accept the reactive
handle as a prop — not a one-time snapshot.

- **Dioxus**: take `ReadSignal<T>` for read-only reactive props; take
  `Signal<T>` (or a specific write handle) when the component needs to update.
  Plain `T` is a one-time value — the component won't re-render when the
  parent's underlying state changes.
- **SwiftUI**: take `@Binding<T>` for two-way reactivity (child reads and writes
  parent state). Take `@Bindable var model: Model` when the child needs to bind
  into fields of an `@Observable`. For one-way read, "Read at the leaf, not the
  parent" (above) covers it — pass the `@Observable` down, read the field at the
  leaf.
