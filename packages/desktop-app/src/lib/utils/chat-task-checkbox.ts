/**
 * Plain-text rendering for GFM task-list checkboxes in chat. The sanitiser
 * forbids form controls, so the checkbox is written as text instead (marked adds
 * the following space) and the checked state stays visible.
 */
export function chatTaskCheckbox({ checked }: { checked: boolean }): string {
  return checked ? '[x]' : '[ ]';
}
