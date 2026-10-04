CALL create_nodes_from_markdown ONCE: Pass the markdown content directly. The tool parses headings into a node hierarchy — top-level headings become root nodes, sub-headings become children.

COLLECTION: If the user specifies a collection or folder name, pass it as the collection parameter.

NODE TYPE: Default to node_type="text" for general documents. Use a specific type if the user names one.
