This skill explains a type and changes it. A type is a schema: its fields, each with a type, allowed values, a default and a description, the relationships it declares, and the type it extends. Its id is the type's name.

READ THE TYPE FIRST: get the schema node and read it as stored. Its `fields` are the effective set, the type's own and then the ones it inherits, and a field's `protection` says whether it is built in. A built-in type's schema also has `custom_fields`, listed apart from `fields`: the fields users added to it. The read also lists `field_references`: the saved queries, plays and title templates that name each of the type's own fields, and what was searched to find them.

SAY WHICH CASE THE TYPE IS IN BEFORE OFFERING ANY CHANGE:
- A built-in type (`isCore`) has two lists. `fields` is what NodeSpace ships: a field whose `protection` is `core` or `system` cannot be changed or removed, and one whose `protection` is `user` (`priority` and the dates on a task) can be, after what names it is dealt with. `custom_fields` is where a field of the user's own goes, under its own name with no prefix, and it is the one list a field can be added to; the user's own values can also be added to a field with `extensible` set.
- A type the user defined has no `custom_fields`: every field of it is the user's own and is declared in `fields`. Its own fields can be added, changed and removed. A field it inherits from a built-in type is as closed as it is there. No type takes a value its schema does not declare, so a value with no field is refused, never kept loose on a node.
- A user's type that extends a built-in one has both: its own fields are the user's and the inherited ones are closed.
Say the case in one sentence, then what the user can do in it. A request to change or remove a `core` or `system` field of a built-in type is explained and not attempted: say that it is built in and offer a field of their own or a value instead.

<!-- include: add-a-field-to-a-built-in-type -->

ANSWER FROM THE STORED DEFINITION: the schema read is the source of truth, and the read comes first for every question about the type, however short; never skip it. The description the chat opened with holds only until the type is changed in this chat, and the read supersedes it after that. Never give a type's parent, fields or values from memory. Name the type it extends exactly as the stored schema gives it, or say it extends nothing when it names none. When the read and the opening description agree, restate the description and do not contradict it.

EXPLAINING WRITES NOTHING: answer what a field means, which values it takes, what is required, what the type extends and what may be changed from the stored schema, and name only what it declares. Say when the schema gives no description for a field instead of guessing one.

A WELL-FORMED FIELD: a snake_case name, the field type that fits what it holds, a default where one is needed, a value list in place of free text where the values are known, and a relationship in place of a list of ids when the field refers to another type of node. A name that is already a field of the type (in `fields` or in `custom_fields`), of a type it extends, or a relationship of the type is refused with the reason, in either direction: use the existing field, or ask for a different name. Take the fields from what the user asked for.

CHECK WHAT A CHANGE WOULD BREAK: before renaming a field or removing one, read the `field_references` for it and tell the user each saved query, play and title template that names it, and where, with what was searched. A result of none rules out only what was searched. If the read has no `field_references`, search the saved queries and plays for the field's name and say that was a search by name. A rename or a removal that a saved query or play names is refused with that list and changes nothing: tell the user, and repeat the call with `force` set once they agree. A rename is a rekey of every node's stored value and breaks each name that is not changed with it. A field the title template names cannot be renamed until the template is set to one that does not name it, then set again with the new name. A removal drops the declaration and leaves the values nodes already hold in place: they are no longer a field of any node, nothing reads them, and a node that holds one can still be updated. Adding the same name again picks them up, and is refused when a stored value does not fit the new declaration. A template that names the field blocks the removal.

SAY WHAT WILL CHANGE, THEN ASK: before writing, say in one or two plain sentences what the type will look like afterwards and what the change affects, and ask for the yes; write only after the user has agreed or has asked for exactly that change. A field added to a built-in type exists on every node of the type, so it too waits for the yes. A change that adds a value to a list affects nothing that exists, and still waits for the yes.

<!-- include: edit-dont-recreate -->

<!-- include: add-enum-values -->

RENAME OR RELABEL: renaming a field changes its stored name and breaks what names it; relabelling changes only the label people see, and breaks nothing. Never describe what a rename does from memory: read the type first, and when something names the field, say what before asking which the user wants.

REPAIR A REJECTED WRITE: a refused write changes nothing and says why, naming the field at fault. Fix exactly that, write again, and do not say the change is made until a write has succeeded.
