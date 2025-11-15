/**
 * ESLint rule: enforce-nodetype
 *
 * Enforces use of `nodeType` instead of `type` for Node-related interfaces.
 *
 * This rule warns when:
 * - An interface named "Node" or ending with "Node" has a property named "type"
 * - Suggests using "nodeType" instead for clarity and consistency
 *
 * Exceptions: Allows "type" in interfaces clearly related to:
 * - Schemas and fields (e.g., SchemaField, EventType)
 * - AST nodes (e.g., ASTNode, DocumentNode - markdown parsing structures)
 * - Test helpers (e.g., TestNode, TestHierarchyNode - test fixtures)
 *
 * Related: Issue #507, identifier-naming-conventions.md
 */

export default {
  meta: {
    type: 'suggestion',
    docs: {
      description: 'Enforce nodeType instead of type for Node interfaces',
      category: 'Best Practices',
      recommended: true
    },
    messages: {
      useNodeType: 'Use "nodeType" instead of "type" for Node interfaces. See identifier-naming-conventions.md for rationale.'
    },
    schema: []
  },

  create(context) {
    return {
      TSInterfaceDeclaration(node) {
        const interfaceName = node.id.name;

        // Check if this is a NodeSpace domain interface
        // Match: "Node", "*Node", but NOT "*NodeResult" or exceptions
        // Exceptions: Schema/Field contexts, AST nodes (extends ASTNode), Test helpers
        const isNodeInterface = (
          interfaceName === 'Node' ||
          (interfaceName.endsWith('Node') && !interfaceName.endsWith('NodeResult'))
        ) &&
        !interfaceName.includes('Schema') &&
        !interfaceName.includes('Field') &&
        !interfaceName.includes('AST') &&        // Exclude ASTNode itself
        !interfaceName.startsWith('Test');       // Exclude test helpers

        if (!isNodeInterface) {
          return;
        }

        // Additional check: Skip if interface extends ASTNode (markdown parsing structures)
        const extendsASTNode = node.extends?.some(
          ext => ext.expression?.name === 'ASTNode'
        );

        if (extendsASTNode) {
          return; // Skip markdown AST nodes
        }

        // Check if interface has a property named "type"
        for (const member of node.body.body) {
          if (member.type === 'TSPropertySignature' && member.key) {
            const propertyName = member.key.type === 'Identifier' ? member.key.name : null;

            if (propertyName === 'type') {
              context.report({
                node: member,
                messageId: 'useNodeType'
              });
            }
          }
        }
      }
    };
  }
};
