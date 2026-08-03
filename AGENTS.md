# Architecture Principles

- Keep the data model as simple as possible.
- Do not persist fields or concepts that can be derived reliably from existing data.
- Add tables, columns, and abstractions only when they represent necessary source data or enforce a concrete invariant.

# UI Design Principles

- Make selection and focus unambiguous. Only one element may be focused on a screen at a time.
- Use spacing judiciously. A single cell is significant in a TUI, and dense layouts are appropriate when they remain clear.
- In forms, render each field label directly adjacent to its input with no blank row between them.
- Separate distinct form fields with exactly one blank row by default. Only deviate when a screen explicitly specifies a denser layout.
- Center button labels horizontally.
- Respect the semantic design tokens. Do not introduce colors outside the token system once it is defined.
- Use only the shared grayscale palette in `src/theme.ts`, with steps `100`, `200`, `300`, `400`, `500`, `600`, `700`, `800`, and `900`. Never add an arbitrary color literal; every foreground, background, border, and cursor color must reference one of these tokens.
- Give every screen a clear visual and informational structure.
- Design keyboard-first interactions with predictable navigation and actions.
- Use progressive disclosure so routine workflows stay simple and advanced details appear only when needed.
