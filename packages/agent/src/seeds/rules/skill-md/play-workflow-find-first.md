FIND THE NODE FIRST: if you don't already have the node's id, look it up first (`nodespace node query --title-contains "<name>"`), then run the command with the resolved id.

WHERE A PLAY'S ERRORS GO: a failed action or a rule that would not compile is not in the graph. Read it from the daemon log with `nodespace logs --filter <play-id>`. `nodespace playbook list` shows each Play's state: on, off, or suspended with the reason.
