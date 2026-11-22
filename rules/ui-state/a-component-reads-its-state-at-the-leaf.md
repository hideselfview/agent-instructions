---
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.tsx'
blocking: false
---

## A component reads its state at the leaf

This rule holds in every UI framework. The framework records which view reads
which value and redraws that view when the value changes, so where a view reads
a value decides what redraws.

### Read at the leaf

Pass the observable object (a store, a service, a view model) down, and let the
leaf view read the field it shows. A parent that reads the field and passes the
value down redraws on every change, though it doesn't use the value.

- SwiftUI: pass the `@Observable` object, and read `store.field` in the leaf's
  `body`.
- Compose: pass the `State<T>` or a lambda that reads it, and read it in the
  leaf.

### Pass a handle when the child changes the value

A child that changes a value its parent owns takes a handle to the parent's
state, not a copy of the value: a change to a copy never reaches the parent.

- SwiftUI: `@Binding<T>` for one value; `@Bindable var model: Model` to bind to
  the fields of an `@Observable`.
- Compose: the value together with an `onValueChange` callback.

### Reach each piece of state one way

A component reaches a piece of state either as a parameter or from the
environment (SwiftUI `@Environment`, Compose `CompositionLocal`), never both.
Both make it unclear whether the component is driven by its parameters or by its
context, and make every test and preview supply the state twice.

- Example: `@Environment(UiStore.self)` together with a `@Binding var foo` to a
  field of the same `UiStore`.
- Fix it by removing the environment read, so the component takes parameters
  only.
  - Reading only from the environment instead makes every test build a real
    environment value.
- A parameter is right even when the environment holds the state, when:
  - a framework API requires one (`NavigationStack(path:)`,
    `Picker(selection:)`, a text field the caller controls);
  - the state is a parent's local state, not in the environment;
  - the component takes parameters only and never reads the environment.
