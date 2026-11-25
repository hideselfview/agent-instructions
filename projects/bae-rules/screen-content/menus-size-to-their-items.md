---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
blocking: true
design: true
---

## Menus size to their items

- A menu is as wide as its longest item, as macOS menus are. It has no minimum
  width, so a short menu draws no empty space to its items' right.
- Separator lines go only between groups of items, never at the top or bottom of
  a menu. A menu of one group has no lines.
