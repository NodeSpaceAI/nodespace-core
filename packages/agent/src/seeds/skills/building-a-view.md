This skill explains a saved view and changes it. A view is a saved query: the type it lists (`target_type`), `filters` that are all applied together, `sorting`, a `limit` and a `view_config` for how it is shown.

READ THE VIEW FIRST: get the query node and read its target type, filters and sorting as stored. Say which type the view is of: a view of a subtype and a view of its base type show different fields for the same node.

EXPLAINING WRITES NOTHING: to say what the view lists, put its filters in a sentence in the user's words. To say what it can be filtered on, get the schema node of the target type (its id is the type name) and list its fields with their types and allowed values, then its relationships with the type each one reaches. For what is reachable through a relationship, get that type's schema the same way. Name only what the schema declares.

WHAT A FILTER CAN BE:
- `property`: a field of the type. Operators: equals, contains, gt, lt, gte, lte, in (value is a list), exists. A date field takes a `YYYY-MM-DD` value, or `relative_date` with `anchor: "today"` and `offset_days` (negative for days before) in place of `value`.
- `content`: the node's text, with contains.
- `metadata`: created_at, modified_at, node_type.
- `relationship`: the node is connected through `path` to the node `node_id`, or to the current person with `anchor: "current_person"`.
- `related`: some node reached through `path` matches the nested `filter`.
- `permitted`: keeps nodes a rule would let `property` be set to `value`.
- `negate: true` on any filter keeps the nodes the condition does not hold for.

SORTING AND LAYOUT: sort by a field of the type or by created_at, modified_at, title; each item has `field` and `direction` (asc or desc). `limit` caps the rows. `view_config` holds `lastView` (list, table, kanban) and, for a board, `kanban.groupBy` naming an enum field.

WHY A NODE IS OR IS NOT IN THE VIEW: get the node and go through the view's filters one at a time, saying which it meets and which it fails. Check its type first: a node of another type is never in the view.

SAY WHAT WILL CHANGE, THEN ASK: before changing the view, tell the user in one or two plain sentences what it will list differently, and write only after they agree.

WRITE THE WHOLE LIST: a change to `filters` or `sorting` replaces the list, so send every item the view should keep along with the new one. Change only the fields the user asked about. <!-- include: graph-editing-update-call -->

REPAIR A REJECTED WRITE: a refused write changes nothing and names the field at fault with the fields that exist. Use one of those, write again, and do not say the change is made until a write has succeeded.
