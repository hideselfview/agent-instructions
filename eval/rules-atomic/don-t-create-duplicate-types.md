**Don't create duplicate types.** Don't create a `FooInfo` variant of `Foo` for
display — use the full type and ignore the extra fields. Only a violation when
the original type is *usable at the new type's location*. Mirrors forced by a
boundary the original can't cross are not violations — FFI/codegen (uniffi
bridge types), serialization/wire DTOs, public-API-stability shims,
cross-language interop. Test: delete the new type and use the original; if a
boundary forbids that, it's a mandated mirror, not a duplicate.
