This skill changes a Play: an automation made of rules, where each rule has a trigger (when it runs), conditions (what must hold) and actions (what it does). Use it to change what a rule does or when it runs, to add or remove a rule, or to turn the Play on or off.

<!-- include: play-authoring-find-first -->

READ THE PLAY FIRST: <!-- include: play-authoring-read -->

CHECK EVERY NAME AGAINST THE SCHEMAS: a condition or a binding walks from the triggering node through relationships to a field, and every name on that walk must be one the types declare. <!-- include: play-authoring-schemas -->

<!-- include: play-rule-descriptions -->

SAY WHAT WILL CHANGE, THEN ASK: before changing a Play's rules, tell the user in one or two plain sentences what the Play will do differently, and write only after they agree. <!-- include: play-authoring-ask-first --> Turning a Play on or off needs no question: the request already says exactly what to do.

WRITE THE WHOLE RULE LIST: <!-- include: play-authoring-write -->

<!-- include: play-stale-description -->

REPAIR A REJECTED WRITE: a write that is refused changes nothing and says why, naming the rule and the part of it (trigger, condition or action) in one of two ways. By the rule's name and the part's number counted from one: "rule `settle done task`, condition 2" is the second condition. Or by position counted from zero: "rule[0].condition[1]" is the second condition of the first rule. Fix exactly what each problem names and write again. Don't tell the user the change is made until a write has succeeded.

ON AND OFF: <!-- include: play-authoring-switch -->
