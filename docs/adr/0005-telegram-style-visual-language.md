# ADR-0005: A Telegram-style visual language in Asiri's palette

* **Status:** Accepted
* **Date:** 2026-11-01
* **Relates to:** architecture §13 (UI and UX design)

## Context

The product owner chose a Telegram-style interface: patterned chat wallpaper, bubbles with tails,
denser rows, colourful sender names in groups, reaction bursts and swipe-to-reply. §13.8 asks for
an identity that is *clearly distinct from existing messenger icons* and suggests the palette
Night `#14122B`, Indigo `#4F46E5` and Amber `#F5A524`.

## Decision

* Adopt the interaction model and density of Telegram's Mac app, drawn entirely from original
  assets: the wallpaper is a procedurally generated pattern of Asiri's own glyphs (keyholes,
  sparks, waves) rather than any third-party doodle set, and bubble tails are our own path.
* Use Asiri's palette: indigo outgoing bubbles, amber for unread badges and the Verified seal,
  per-person name colours from a fixed, WCAG-checked set of eight hues.
* Light and dark themes both ship; status never relies on colour alone (ticks differ in shape).
* All motion uses short springs and honours Reduce Motion and Reduce Transparency (§13.9).

## Consequences

The look is familiar to anyone who has used a modern messenger, but nothing in it is borrowed
artwork, which keeps the app's identity distinct and avoids trade-dress problems.

## Amendment (2026-11-03)

As built, the chat wallpaper scatters SF Symbols (locks, keys, moons, sparks and more) rather
than glyphs drawn for Asiri, and the app honors Reduce Motion but has no separate treatment for
Reduce Transparency yet. Bubble tails, the icon and the install window's picture are Asiri's own.
