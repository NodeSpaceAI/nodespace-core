import { createLogger } from '$lib/utils/logger';

const log = createLogger('ConflictNotifications');

const MAX_NOTIFICATIONS = 10;

export interface ConflictNotification {
  id: string;
  nodeId: string;
  message: string;
  conflictType:
    | 'version-mismatch'
    | 'deleted-node'
    | 'child-transfer-failure'
    | 'write-failure'
    | 'conflict-journal';
  createdAt: number;
}

class ConflictNotificationStore {
  notifications = $state<ConflictNotification[]>([]);

  add(notification: Omit<ConflictNotification, 'id' | 'createdAt'>): string {
    const now = Date.now();
    const id = `conflict-${now}-${Math.random().toString(36).slice(2, 7)}`;
    const entry: ConflictNotification = { ...notification, id, createdAt: now };
    const updated = [...this.notifications, entry];
    this.notifications = updated.length > MAX_NOTIFICATIONS ? updated.slice(1) : updated;
    log.debug('Conflict notification added', { id, nodeId: notification.nodeId });
    return id;
  }

  dismiss(id: string): void {
    this.notifications = this.notifications.filter((n) => n.id !== id);
  }

  dismissAll(): void {
    this.notifications = [];
  }
}

export const conflictNotifications = new ConflictNotificationStore();
