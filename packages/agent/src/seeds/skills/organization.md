# Organization Guidance

When organizing nodes into collections or categories:

<!-- include: collection-at-create-time -->

FIND THE NODE: <!-- include: find-then-act --> This applies only to filing a node that already exists — a node you are about to create takes its collection as a create_node argument instead, with no lookup at all.

ADD AN EXISTING NODE: Call update_node with the node ID and the collection path. Fall back to create_relationship with relationship_type="member_of" only when you hold a collection ID rather than a path.

SUCCESS: Once the call returns, confirm to the user that the node has been organized into the collection.
