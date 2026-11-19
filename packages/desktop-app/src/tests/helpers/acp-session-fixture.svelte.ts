/**
 * A scripted ACP session for the viewer tests: the live state a running
 * session would hand the viewer, with every control recorded.
 */
import type {
  AcpPermissionOutcome,
  AcpPermissionRequest,
  AcpSessionSource,
  AcpSessionUsage
} from '$lib/types/acp-session';

export class FixtureAcpSession implements AcpSessionSource {
  pendingPermission = $state<AcpPermissionRequest | null>(null);
  turnRunning = $state(false);
  usage = $state<AcpSessionUsage | null>(null);
  error = $state<string | null>(null);

  prompts: string[] = [];
  answers: Array<{ requestId: string; outcome: AcpPermissionOutcome }> = [];
  cancelled = 0;
  ended = 0;
  resumed = 0;

  async sendPrompt(text: string): Promise<void> {
    this.prompts.push(text);
  }
  async cancelTurn(): Promise<void> {
    this.cancelled += 1;
  }
  async answerPermission(requestId: string, outcome: AcpPermissionOutcome): Promise<void> {
    this.answers.push({ requestId, outcome });
  }
  async endSession(): Promise<void> {
    this.ended += 1;
  }
  async resumeSession(): Promise<void> {
    this.resumed += 1;
  }
}

/** The four options a Claude-style agent offers: two persistent, two once-only. */
export const FOUR_OPTION_REQUEST: AcpPermissionRequest = {
  requestId: 'req-1',
  toolCallId: 'call_1',
  toolTitle: 'Run `rm -rf build`',
  toolKind: 'execute',
  options: [
    { optionId: 'allow-always', name: 'Always allow', kind: 'allow_always' },
    { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
    { optionId: 'reject-always', name: 'Always reject', kind: 'reject_always' },
    { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
  ]
};
