import type { NodespaceExtension } from '@nodespace/extension-api';

/** A build's extension entry: the module `NODESPACE_EXTENSIONS` points at. */
export default [
  { id: 'sample-extension', apiVersion: 1 }
] satisfies NodespaceExtension[];
