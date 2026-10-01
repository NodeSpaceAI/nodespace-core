/**
 * UI building blocks for extension components (ADR-082 §2.6), imported as
 * `@nodespace/extension-api/ui`: core's buttons, cards, dialogs, inputs and
 * badges, and the `focusTrap` action for hand-rolled modals. Part of the
 * versioned host API; see the compatibility policy in `./index.ts`.
 *
 * `Dialog` is a namespace, used as `<Dialog.Root>`, `<Dialog.Content>` and so on.
 */

import * as Dialog from '$lib/components/ui/dialog';

export { Dialog };
export { Badge } from '$lib/components/ui/badge';
export { Button } from '$lib/components/ui/button';
export {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle
} from '$lib/components/ui/card';
export { Input } from '$lib/components/ui/input';
export { focusTrap } from '$lib/actions/focus-trap';
