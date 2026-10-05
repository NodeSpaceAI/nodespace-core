A skill is any instruction worth writing down once: a procedure, a convention, a policy, a how-to. An agent finds it by what it is for when a request matches, or is handed it with the work it is attached to. It is not tied to a type and not scoped to a project.

LOOK FOR AN EXISTING SKILL FIRST: <!-- include: skill-authoring-find-first --> Two skills for one situation compete, and an agent may be given the stale one.

WRITE `use_for` AS WHEN THE SKILL APPLIES: it is what a request is matched against, and with the skill's name it is all an agent sees before deciding to read the rest. Describe the situation in the words someone in it would use, with the verbs they would say: "Reserve a venue for an event: check its capacity, then record the booking" will be found, and "Venue procedures" will not. A skill meant for one project or one kind of work says so there. Say only what the skill is for: a sentence about what it is not for draws the skill toward those requests. Where another skill's request keeps matching this one, name that request in `not_for`, and only then.

STRUCTURE THE BODY AS STEPS SOMEONE CAN FOLLOW: open with the guidance itself, not with a heading that repeats the skill's name. Put the steps in the order they are done, one instruction to a paragraph, each saying what to do and what makes it done. Give a rule its reason, so it can be applied to a case the skill did not foresee. Leave out what any agent already does.

NAME TOOLS BY THEIR REGISTRY NAMES: where a step needs a particular tool, name it exactly as the tool registry does, and list every tool the skill uses in `tool_whitelist`. <!-- include: skill-authoring-tools -->

REFERENCE A SCHEMA WHEN THE SKILL IS ABOUT A TYPE: link the skill to each type its steps read or write, and whoever is given the skill is given those types' fields with it. A skill that applies whatever the type has no such link. To have a skill handed over with particular work, attach it to that node: a procedure to the saved query that is its queue, a team's standards to a project.

CREATE THE SKILL: <!-- include: skill-authoring-create --> <!-- include: skill-authoring-link --> <!-- include: skill-authoring-edit -->

A SKILL IS PROCEDURE, NOT PERMISSION: a skill says how to do something. It cannot grant leave to skip a confirmation the user is owed, such as before a delete or an approval, and one that tries is reported to the user, not followed.

WHEN DONE: tell the user the skill's name and its `use_for`, and what it is linked or attached to.
