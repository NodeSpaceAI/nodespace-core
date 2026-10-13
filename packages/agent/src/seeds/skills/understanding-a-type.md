This skill explains a type and changes it. A type is a schema: its fields, each with a type, allowed values, a default and a description, the relationships it declares, and the type it extends. Its id is the type's name.

READ THE TYPE FIRST: get the schema node and read it as stored. Its fields are the effective set, the type's own and then the ones it inherits, and a field's `protection` says whether it is built in. The read also lists `field_references`: the saved queries, plays and title templates that name each of the type's own fields, and what was searched to find them.

SAY WHICH CASE THE TYPE IS IN BEFORE OFFERING ANY CHANGE:
- A built-in type (`isCore`) is closed. Its declared fields cannot be changed or removed. What may be added is a field of the user's own, whose name carries a prefix such as `custom:`, and the user's own values to a list that accepts them, which is a field with `extensible` set.
- A type the user defined is open. Its own fields can be added, changed and removed. A field it inherits from a built-in type stays closed.
- A user's type that extends a built-in one has both: its own fields are open and the inherited ones are closed.
Say the case in one sentence, then what the user can do in it. A request to change or remove a declared field of a built-in type is explained and not attempted: say that it is built in and offer a field of their own or a value instead.

EXPLAINING WRITES NOTHING: answer what a field means, which values it takes, what is required, what the type extends and what may be changed from the stored schema, and name only what it declares. Say when the schema gives no description for a field instead of guessing one.

A WELL-FORMED FIELD: a snake_case name, the field type that fits what it holds, a default where one is needed, a value list in place of free text where the values are known, and a relationship in place of a list of ids when the field refers to another type of node. A name that is already a field of the type, or of a type it extends, is refused with the reason, and so is a declared field's name behind a prefix (`custom:status` beside `status`): use the existing field, or ask for a different name. Take the fields from what the user asked for.

CHECK WHAT A CHANGE WOULD BREAK: before renaming a field or removing one, read the `field_references` for it and tell the user each saved query, play and title template that names it, and where, with what was searched. A result of none rules out only what was searched. If the read has no `field_references`, search the saved queries and plays for the field's name and say that was a search by name. A rename or a removal that something names is refused with that list and changes nothing: tell the user, and repeat the call with `force` set once they agree. A rename is a rekey of every node's stored value and breaks each name that is not changed with it. A removal drops the declaration and leaves the values nodes already hold stored but unread, and a template that names the field blocks it.

SAY WHAT WILL CHANGE, THEN ASK: before writing, say in one or two plain sentences what the type will look like afterwards and what the change affects, and write only after the user agrees. A change that adds a field or a value affects nothing that exists, and still waits for the yes.

<!-- include: edit-dont-recreate -->

<!-- include: add-enum-values -->

RENAME OR RELABEL: renaming a field changes its stored name and breaks what names it; relabelling changes only the label people see, and breaks nothing. Never describe what a rename does from memory: read the type first, and when something names the field, say what before asking which the user wants.

REPAIR A REJECTED WRITE: a refused write changes nothing and says why, naming the field at fault. Fix exactly that, write again, and do not say the change is made until a write has succeeded.
