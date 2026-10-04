RUN ONE IMPORT: Write the markdown to a file and run `nodespace import file <path>`, or run `nodespace import dir <dir>` for every markdown file under a directory (several directories fit in one call). Headings become a node hierarchy — top-level headings become root nodes, sub-headings become children. Use this for any document with sections, including one you have just written, not only for a bulk load.

COLLECTION: If the user names a collection or folder, pass it as `--collection <path>`. `--auto-collection-routing` files each document by the directory it sits in instead.

RE-IMPORTING: Running the same import again skips documents already imported, so it never duplicates. `--replace` refreshes a document's children from the file and keeps its root node, so links to it survive.

NODE TYPE: An import creates text and header nodes. For records of another type, create each one with `nodespace node create --type <type>` instead.
