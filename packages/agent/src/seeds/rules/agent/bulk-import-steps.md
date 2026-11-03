CALL create_nodes_from_markdown ONCE: Pass the markdown content directly. The tool parses headings into a node hierarchy — top-level headings become root nodes, sub-headings become children.

COLLECTION: If the user specifies a collection or folder name, pass it as the collection parameter.

NODE TYPE: Default to node_type="text" for general documents. Use a specific type if the user names one.


MARKDOWN SOURCE OR SINGLE NODES: a markdown source, such as a document, a pasted page or a copied Google Doc, goes in with create_nodes_from_markdown, not node by node. When you create individual nodes instead, use the node type that matches the syntax and keep that type's own prefix in the content: a header keeps "## " and its level, an ordered-list node keeps "1. ", a quote-block keeps "> ", a checkbox keeps its box. A plain list item is a text node under its header with no "- " or "* " in front of it; the outline supplies the bullet. Never type markdown syntax into a text node.