---
name: frontend-design
description: Create distinctive, production-grade browser frontend interfaces with high design quality. Use when building or revising web components, pages, dashboards, landing pages, games, or browser applications where visual design, accessibility, responsive layout, and interaction polish matter. Do not use for Rust TUI or terminal-only interface work.
---

Use this skill to build web components, pages, and applications that are production-grade, accessible, performant, and visually specific to the user's context. Avoid generic AI aesthetics by making a clear design argument: every choice should support the product, audience, content, and interaction model.

Distinctive design is not novelty for its own sake. Bold aesthetics must not break usability.

## 1. Design Thinking

Before coding, form a compact design brief. If the request is underspecified, infer sensible defaults from the product and audience; ask clarifying questions only when missing information would change the framework, workflow, brand, or accessibility requirements.

Design brief checklist:

- **Purpose and user**: What job does the interface perform, and who relies on it?
- **Primary action**: What should the user understand or do first?
- **Content hierarchy**: What information deserves emphasis, density, or quiet?
- **Context cues**: What visual language belongs to this domain, audience, or brand?
- **Aesthetic direction**: Choose one coherent point of view, such as editorial restraint, tactile craft, technical instrument, playful object, civic utility, cinematic depth, or luxury minimalism. Treat these as starting points, not templates.
- **One memorable mechanic**: Define a specific design move people will remember: a navigation model, typographic system, data visualization, material treatment, transition, or layout structure.
- **Constraints**: Framework, existing design system, assets, responsiveness, localization, accessibility, browser support, and performance budget.

Convert abstract goals into mechanics:

- Generic: centered hero, soft gradient background, interchangeable rounded cards, stock icons, vague "modern" typography, decoration unrelated to the product.
- Distinctive: a marine research archive using tide-chart grids, kelp-and-ink color roles, coordinate-style section labels, dense specimen cards, map-like dividers, and a subtle current animation that becomes static under reduced motion.

## 2. Aesthetic Guidance by Dimension

Use common choices only when they serve a reason: readability, performance, localization, brand consistency, or user expectation. Do not rely on blacklists of named fonts, colors, or patterns. The issue is not that a choice is popular; the issue is using it on autopilot.

- **Typography**
  - Choose type for voice and reading conditions: editorial, technical, utilitarian, luxurious, playful, archival, or raw.
  - Pair display and body styles intentionally. If using external fonts, keep the set small and provide strong fallbacks.
  - If system fonts are required, make them feel designed through scale, weight, tracking, line height, spacing, contrast, and composition.
  - Preserve legibility for long text, small labels, numerals, and localized strings.

- **Color and theme**
  - Build palettes from the product context, not from fashionable defaults.
  - Use CSS variables for semantic roles: background, surface, text, muted text, accent, danger, success, border, focus.
  - Prefer clear dominance and purposeful accents over evenly distributed color noise.
  - Support both aesthetic richness and WCAG contrast requirements.

- **Spatial composition**
  - Establish a strong hierarchy before adding ornament.
  - Use asymmetry, overlap, diagonal flow, dense grids, or generous negative space when they clarify the experience.
  - Break the grid only with intent; alignment, rhythm, and proximity should still guide the eye.
  - Design responsive layouts as separate compositions for mobile, tablet, and desktop, not as an afterthought.

- **Motion and interaction**
  - Use motion to explain state, guide attention, or create one memorable moment.
  - Prefer CSS transitions and transforms for simple effects; use the project's motion library only when it adds clear value.
  - One well-orchestrated reveal is better than many unrelated animations.
  - Hover, active, loading, empty, error, disabled, and focus states should all feel designed.

- **Materials, backgrounds, and detail**
  - Create atmosphere with context-specific textures, pattern, depth, illustration, borders, shadows, gradients, or SVG details.
  - Keep effects subordinate to content. Texture must not reduce readability.
  - Custom cursors, parallax, canvas, and decorative overlays are optional enhancements, not core interaction requirements.

- **Component character**
  - Make controls recognizable first, distinctive second.
  - Give buttons, inputs, cards, navigation, tables, and dialogs a shared detail language: radius, stroke, shadow, icon style, spacing, and focus treatment.
  - Avoid placeholder content that makes the interface feel generic. Use plausible domain-specific labels and data.

## 3. Accessibility and Performance Guardrails

A visually ambitious interface is not production-grade unless it remains usable.

Accessibility requirements:

- Meet **WCAG 2.1 AA** contrast: at least 4.5:1 for normal text, 3:1 for large text, and 3:1 for UI components and focus indicators.
- Use semantic HTML: real buttons for actions, links for navigation, labels for inputs, headings in order, lists/tables where appropriate.
- Ensure complete keyboard access with logical tab order, visible focus styles, no keyboard traps, and no hover-only functionality.
- Do not rely on color alone; pair status colors with text, icons, shape, or labels.
- Provide meaningful alt text for informative images and hide decorative visuals with `aria-hidden="true"`.
- For animated or split text, preserve a readable accessible name; do not make screen readers announce fragmented decorative spans.
- Custom cursors and parallax must be decorative, pointer-device only, and screen-reader safe. They must not hide essential content, replace native focus feedback, or be required to complete a task.
- Respect `prefers-reduced-motion`: disable parallax, cursor trails, large transforms, scroll-linked motion, autoplay loops, and rapid flashing; replace with static states or minimal opacity transitions.
- Use touch targets around 44 by 44 CSS pixels where practical, and prevent horizontal overflow on small screens.

Default performance budget unless the project specifies otherwise:

- Avoid adding decorative libraries when CSS, SVG, or existing utilities can achieve the effect.
- Keep design-specific JavaScript small: target under 25 KB gzipped for a component and under 150 KB gzipped for a full page.
- Keep CSS lean and scoped; avoid large unused frameworks for a single interface.
- Use no more than two font families and four loaded weights by default; use `font-display: swap`.
- Optimize images and media with responsive sizes, compression, lazy loading, and explicit dimensions.
- Animate `transform` and `opacity` when possible; avoid layout-thrashing scroll handlers and expensive continuous effects.
- Target Core Web Vitals quality: LCP under 2.5s, CLS under 0.1, and responsive input on mid-range mobile devices.

## 4. Variety Mechanism

Prevent convergence by rotating aesthetic decisions instead of banning specific names.

At the start of each design, choose a lane from this rotation. If a previous direction exists in the conversation, advance to a different compatible lane and do not repeat the last theme mode, typography class, or surface treatment. If there is no prior direction, choose the lane that best fits the product context; when multiple lanes fit, use the first meaningful noun in the request and map its letter count modulo 6 to pick a starting lane.

1. **Editorial clarity**: paper-like surfaces, strong hierarchy, serif or literary display voice, fine rules, asymmetric reading flow.
2. **Technical instrument**: dark or monochrome shell, tabular numerals, diagram grids, precise controls, mechanical motion.
3. **Tactile craft**: warm palette, material texture, irregular organic shapes, soft depth, hand-finished details.
4. **Civic utility**: high contrast, dense information architecture, restrained color, durable controls, direct language.
5. **Playful object**: saturated accents, chunky forms, rounded geometry, elastic interactions, object-like components.
6. **Luxury restraint**: sparse composition, muted palette, refined type, fine borders, slow subtle transitions.

Vary at least three axes when producing a new design or another pass:

- Theme: light, dark, tinted, monochrome, high contrast.
- Type voice: serif, humanist sans, condensed display, mono, slab, rounded, calligraphic accent.
- Geometry: grid-true, asymmetric, radial, layered, modular, diagrammatic.
- Density: spacious, editorial, dashboard-dense, compact utility.
- Surface: flat ink, paper grain, glass, metal, clay, textile, pixel, luminous.
- Motion: still and precise, elastic, mechanical, cinematic, scroll narrative, ambient.

User brand and accessibility constraints take precedence over the rotation. Use the rotation to avoid defaulting to the same fashionable interface every time.

## 5. Execution

Implement complete, runnable code that matches the selected direction.

- Inspect the existing project before adding files: framework, routing, component conventions, styling method, package availability, design tokens, lint/build scripts.
- Use the user's requested stack. If none is specified, follow the project stack; if there is no project context, deliver self-contained HTML/CSS/JS or the simplest suitable framework.
- Structure the interface with semantic components, clear state, maintainable CSS variables, and responsive layout rules.
- Match complexity to the vision: maximal designs need a coherent system for effects; minimal designs need exact spacing, typography, and interaction polish.
- Include realistic content, empty/loading/error states when relevant, and accessible form behavior.
- Prefer progressive enhancement: the core experience should work without decorative effects.
- Test what you can: build, lint, viewport responsiveness, keyboard navigation, focus visibility, contrast, reduced-motion behavior, console errors, and obvious performance regressions.
- In the final response, briefly state the aesthetic direction, key implementation choices, files changed, and any checks performed.

Commit to one intentional direction and refine every detail until the interface feels specific, usable, accessible, and production-ready.
