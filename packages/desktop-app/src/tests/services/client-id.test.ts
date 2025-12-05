/**
 * Client ID Service Tests
 *
 * Comprehensive test suite for the client ID service that generates and manages
 * unique browser session identifiers for SSE filtering.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { getClientId, resetClientId } from '$lib/services/client-id';

describe('Client ID Service', () => {
  beforeEach(() => {
    // Clear any existing client ID before each test
    if (typeof window !== 'undefined' && window.sessionStorage) {
      window.sessionStorage.clear();
    }
  });

  describe('getClientId() - Browser Environment', () => {
    it('should generate a new client ID when none exists', () => {
      const clientId = getClientId();

      expect(clientId).toBeTruthy();
      expect(typeof clientId).toBe('string');
      expect(clientId.length).toBeGreaterThan(0);
    });

    it('should return a valid UUID format', () => {
      const clientId = getClientId();

      // UUID v4 format: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
      const uuidRegex =
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
      expect(clientId).toMatch(uuidRegex);
    });

    it('should persist client ID in sessionStorage', () => {
      const clientId = getClientId();

      const storedId = window.sessionStorage.getItem('nodespace_client_id');
      expect(storedId).toBe(clientId);
    });

    it('should return the same client ID on subsequent calls', () => {
      const firstCall = getClientId();
      const secondCall = getClientId();
      const thirdCall = getClientId();

      expect(firstCall).toBe(secondCall);
      expect(secondCall).toBe(thirdCall);
    });

    it('should retrieve existing client ID from sessionStorage', () => {
      // Manually set a client ID
      const existingId = 'existing-test-id-12345';
      window.sessionStorage.setItem('nodespace_client_id', existingId);

      const retrievedId = getClientId();

      expect(retrievedId).toBe(existingId);
    });

    it('should not regenerate ID if sessionStorage already has one', () => {
      // First call generates and stores ID
      const firstId = getClientId();

      // Clear and manually set a different ID
      window.sessionStorage.setItem('nodespace_client_id', 'manual-id');

      // Second call should return the manually set ID
      const secondId = getClientId();

      expect(secondId).toBe('manual-id');
      expect(secondId).not.toBe(firstId);
    });

    it('should handle empty string in sessionStorage', () => {
      // Set empty string (edge case)
      window.sessionStorage.setItem('nodespace_client_id', '');

      // Should generate new ID since empty string is falsy
      const clientId = getClientId();

      expect(clientId).toBeTruthy();
      expect(clientId).not.toBe('');
    });

    it('should use crypto.randomUUID for generation', () => {
      // Spy on crypto.randomUUID
      const mockUUID = '12345678-1234-4123-8123-123456789abc';
      const randomUUIDSpy = vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(mockUUID);

      // Clear any existing ID to force generation
      window.sessionStorage.clear();

      const clientId = getClientId();

      expect(randomUUIDSpy).toHaveBeenCalled();
      expect(clientId).toBe(mockUUID);

      randomUUIDSpy.mockRestore();
    });
  });

  describe('resetClientId()', () => {
    it('should remove client ID from sessionStorage', () => {
      // Generate a client ID
      const clientId = getClientId();
      expect(window.sessionStorage.getItem('nodespace_client_id')).toBe(clientId);

      // Reset the client ID
      resetClientId();

      // Should be removed from storage
      expect(window.sessionStorage.getItem('nodespace_client_id')).toBeNull();
    });

    it('should allow generation of new client ID after reset', () => {
      // Generate first ID
      const firstId = getClientId();

      // Reset
      resetClientId();

      // Generate new ID
      const newId = getClientId();

      // Should be different IDs (very high probability with UUIDs)
      expect(newId).not.toBe(firstId);
      expect(newId).toBeTruthy();
    });

    it('should not throw when resetting non-existent client ID', () => {
      // Ensure no client ID exists
      window.sessionStorage.clear();

      // Should not throw
      expect(() => {
        resetClientId();
      }).not.toThrow();
    });

    it('should clear specific client ID key only', () => {
      // Set multiple items in sessionStorage
      window.sessionStorage.setItem('nodespace_client_id', 'test-id');
      window.sessionStorage.setItem('other_key', 'other-value');

      resetClientId();

      // Client ID should be removed
      expect(window.sessionStorage.getItem('nodespace_client_id')).toBeNull();

      // Other items should remain
      expect(window.sessionStorage.getItem('other_key')).toBe('other-value');
    });
  });

  describe('Integration Scenarios', () => {
    it('should handle full lifecycle: generate -> reset -> regenerate', () => {
      // Generate initial ID
      const id1 = getClientId();
      expect(id1).toBeTruthy();

      // Reset
      resetClientId();
      expect(window.sessionStorage.getItem('nodespace_client_id')).toBeNull();

      // Regenerate
      const id2 = getClientId();
      expect(id2).toBeTruthy();
      expect(id2).not.toBe(id1);
    });

    it('should maintain ID across multiple reads without reset', () => {
      const ids = [];
      for (let i = 0; i < 10; i++) {
        ids.push(getClientId());
      }

      // All IDs should be identical
      const uniqueIds = new Set(ids);
      expect(uniqueIds.size).toBe(1);
    });

    it('should generate unique IDs after each reset', () => {
      const ids = new Set<string>();

      for (let i = 0; i < 5; i++) {
        const id = getClientId();
        ids.add(id);
        resetClientId();
      }

      // All IDs should be unique (very high probability with UUIDs)
      expect(ids.size).toBe(5);
    });

    it('should handle rapid reset and regenerate cycles', () => {
      for (let i = 0; i < 20; i++) {
        const id = getClientId();
        expect(id).toBeTruthy();
        resetClientId();
      }

      // Final generation should still work
      const finalId = getClientId();
      expect(finalId).toBeTruthy();
    });
  });

  describe('Edge Cases and Error Handling', () => {
    it('should handle sessionStorage quota exceeded', () => {
      // Mock sessionStorage.setItem to throw quota exceeded error
      const originalSetItem = window.sessionStorage.setItem.bind(window.sessionStorage);
      const mockSetItem = vi.fn(() => {
        throw new Error('QuotaExceededError');
      });
      Object.defineProperty(window.sessionStorage, 'setItem', {
        value: mockSetItem,
        writable: true,
        configurable: true
      });

      resetClientId(); // Clear first

      // Should throw when trying to store
      expect(() => {
        getClientId();
      }).toThrow();

      // Restore
      Object.defineProperty(window.sessionStorage, 'setItem', {
        value: originalSetItem,
        writable: true,
        configurable: true
      });
    });

    it('should handle very long stored client IDs', () => {
      // Store an extremely long ID
      const longId = 'x'.repeat(10000);
      window.sessionStorage.setItem('nodespace_client_id', longId);

      const retrievedId = getClientId();

      // Should retrieve the long ID correctly
      expect(retrievedId).toBe(longId);
      expect(retrievedId.length).toBe(10000);
    });

    it('should handle special characters in stored IDs', () => {
      const specialId = 'test-🚀-emoji-id-©®™';
      window.sessionStorage.setItem('nodespace_client_id', specialId);

      const retrievedId = getClientId();

      expect(retrievedId).toBe(specialId);
    });

    it('should handle whitespace-only stored IDs', () => {
      // Store whitespace-only ID
      window.sessionStorage.setItem('nodespace_client_id', '   ');

      const retrievedId = getClientId();

      // Should return the whitespace ID (truthy in JavaScript)
      expect(retrievedId).toBe('   ');
    });

    it('should handle null values in sessionStorage gracefully', () => {
      // Set item to null (simulating corruption)
      window.sessionStorage.setItem('nodespace_client_id', 'null');

      const retrievedId = getClientId();

      // Should return the string 'null'
      expect(retrievedId).toBe('null');
    });
  });

  describe('Type Safety and Contracts', () => {
    it('should always return a string', () => {
      const clientId = getClientId();

      expect(typeof clientId).toBe('string');
    });

    it('should never return null or undefined', () => {
      const clientId = getClientId();

      expect(clientId).not.toBeNull();
      expect(clientId).not.toBeUndefined();
    });

    it('resetClientId should have void return type', () => {
      const result = resetClientId();

      expect(result).toBeUndefined();
    });

    it('should return consistent type across multiple calls', () => {
      for (let i = 0; i < 5; i++) {
        const clientId = getClientId();
        expect(typeof clientId).toBe('string');
      }
    });
  });

  describe('Concurrency and Race Conditions', () => {
    it('should handle simultaneous calls consistently', () => {
      // Clear storage first
      resetClientId();

      // Call getClientId multiple times simultaneously
      const promises = Array.from({ length: 10 }, () => Promise.resolve(getClientId()));

      return Promise.all(promises).then((ids) => {
        // All should return the same ID
        const uniqueIds = new Set(ids);
        expect(uniqueIds.size).toBe(1);
      });
    });

    it('should handle reset during read operations', () => {
      const id1 = getClientId();

      // Reset and immediately read
      resetClientId();
      const id2 = getClientId();

      // Should be different IDs
      expect(id2).not.toBe(id1);
      expect(id2).toBeTruthy();
    });

    it('should maintain consistency after multiple operations', () => {
      // Complex sequence of operations
      const id1 = getClientId();
      const id1Copy = getClientId();
      expect(id1).toBe(id1Copy);

      resetClientId();
      const id2 = getClientId();
      expect(id2).not.toBe(id1);

      const id2Copy = getClientId();
      expect(id2).toBe(id2Copy);
    });
  });

  describe('Storage Key Constants', () => {
    it('should use correct storage key', () => {
      const clientId = getClientId();

      // Check that the exact key is used
      const storedValue = window.sessionStorage.getItem('nodespace_client_id');
      expect(storedValue).toBe(clientId);

      // Check that wrong keys don't work
      expect(window.sessionStorage.getItem('client_id')).toBeNull();
      expect(window.sessionStorage.getItem('nodespace_id')).toBeNull();
    });

    it('should only interact with nodespace_client_id key', () => {
      // Add other keys
      window.sessionStorage.setItem('unrelated_key_1', 'value1');
      window.sessionStorage.setItem('unrelated_key_2', 'value2');

      // Get client ID
      getClientId();

      // Other keys should remain untouched
      expect(window.sessionStorage.getItem('unrelated_key_1')).toBe('value1');
      expect(window.sessionStorage.getItem('unrelated_key_2')).toBe('value2');
    });
  });

  describe('SSR and Environment Handling', () => {
    it('should work in browser environment', () => {
      // Verify we're in browser mode (Happy-DOM provides window)
      expect(typeof window).toBe('object');
      expect(window.sessionStorage).toBeDefined();

      const clientId = getClientId();
      expect(clientId).toBeTruthy();
      expect(typeof clientId).toBe('string');
    });

    it('should handle sessionStorage operations correctly', () => {
      // Verify sessionStorage is working
      window.sessionStorage.setItem('test_key', 'test_value');
      expect(window.sessionStorage.getItem('test_key')).toBe('test_value');

      // Now test our service
      const clientId = getClientId();
      expect(window.sessionStorage.getItem('nodespace_client_id')).toBe(clientId);
    });

    it('should return test-client in SSR/test environment when window is undefined', () => {
      // Temporarily mock window as undefined to test SSR path
      const originalWindow = globalThis.window;

      // Delete window to simulate SSR environment
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      delete (globalThis as any).window;

      try {
        const clientId = getClientId();

        // Should return the test client ID
        expect(clientId).toBe('test-client');
      } finally {
        // Restore window
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (globalThis as any).window = originalWindow;
      }
    });

    it('should handle resetClientId gracefully when window is undefined', () => {
      // Temporarily mock window as undefined to test SSR path
      const originalWindow = globalThis.window;

      // Delete window to simulate SSR environment
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      delete (globalThis as any).window;

      try {
        // Should not throw
        expect(() => {
          resetClientId();
        }).not.toThrow();
      } finally {
        // Restore window
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (globalThis as any).window = originalWindow;
      }
    });
  });

  describe('UUID Generation', () => {
    it('should generate different UUIDs for different sessions', () => {
      const uuids = new Set<string>();

      // Generate 10 different sessions
      for (let i = 0; i < 10; i++) {
        resetClientId();
        const uuid = getClientId();
        uuids.add(uuid);
      }

      // All should be unique
      expect(uuids.size).toBe(10);
    });

    it('should generate valid UUID v4 format consistently', () => {
      const uuidRegex =
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

      for (let i = 0; i < 5; i++) {
        resetClientId();
        const uuid = getClientId();
        expect(uuid).toMatch(uuidRegex);
      }
    });

    it('should use crypto API for UUID generation', () => {
      // Spy to verify crypto API is called
      const spy = vi.spyOn(globalThis.crypto, 'randomUUID');

      resetClientId();
      getClientId();

      expect(spy).toHaveBeenCalled();

      spy.mockRestore();
    });
  });

  describe('State Persistence', () => {
    it('should persist across multiple function calls', () => {
      const id = getClientId();

      // Call many times
      for (let i = 0; i < 100; i++) {
        expect(getClientId()).toBe(id);
      }
    });

    it('should survive sessionStorage modifications', () => {
      const originalId = getClientId();

      // Add more items to sessionStorage
      for (let i = 0; i < 10; i++) {
        window.sessionStorage.setItem(`key_${i}`, `value_${i}`);
      }

      // Client ID should still be the same
      expect(getClientId()).toBe(originalId);
    });

    it('should handle sessionStorage being cleared elsewhere', () => {
      const id1 = getClientId();

      // Simulate external code clearing storage
      window.sessionStorage.clear();

      // Next call should generate new ID
      const id2 = getClientId();

      expect(id2).not.toBe(id1);
      expect(id2).toBeTruthy();
    });
  });
});
