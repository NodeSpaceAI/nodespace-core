**Title template:** `content` is a node's name for entity types (`Customer`, `Person`, `Invoice`) — for a node created without a parent, NodeSpace surfaces it as the title automatically. Only markdown primitives (`text`, `header`, `quote-block`, `code-block`, etc.) use `content` as a prose body instead of a name. Three cases:
- **Single-field identity** — e.g. `Customer`: one field's value is the whole title. Put it directly in `content`; don't set `title_template`, and don't add a separate field (e.g. `company_name`) that duplicates it.
- **Composed identity** — e.g. `Person` (`first_name` + `last_name`): no single field holds the full title, so assemble one with `title_template: "{first_name} {last_name}"`, using `{field_name}` placeholders — every placeholder must be a defined field.
- **Markdown primitive** — `text`, `header`, etc.: `content` is prose, not a name; `title_template` doesn't apply.

Use `title_template` only to assemble a title from two or more fields. If one field already holds the whole identity, that value belongs in `content` alone.
