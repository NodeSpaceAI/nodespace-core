/**
 * Date Formatting Utilities
 *
 * Centralized date formatting logic for tab titles and date displays.
 * Single source of truth for date-related string formatting across the app.
 */

/**
 * Normalize a date to midnight local time (ignore time components)
 */
export function normalizeDate(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

/**
 * Format a date as a human-readable title for tabs
 * Returns "Today", "Tomorrow", "Yesterday", or "YYYY-MM-DD"
 *
 * @param date - The date to format
 * @returns Human-readable date string
 */
export function formatDateTitle(date: Date): string {
  const today = new Date();
  const tomorrow = new Date(today);
  tomorrow.setDate(tomorrow.getDate() + 1);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);

  const targetDate = normalizeDate(date);
  const todayNormalized = normalizeDate(today);
  const tomorrowNormalized = normalizeDate(tomorrow);
  const yesterdayNormalized = normalizeDate(yesterday);

  if (targetDate.getTime() === todayNormalized.getTime()) return 'Today';
  if (targetDate.getTime() === tomorrowNormalized.getTime()) return 'Tomorrow';
  if (targetDate.getTime() === yesterdayNormalized.getTime()) return 'Yesterday';

  // Return YYYY-MM-DD format for all other dates
  const year = targetDate.getFullYear();
  const month = String(targetDate.getMonth() + 1).padStart(2, '0');
  const day = String(targetDate.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/**
 * Format a date as YYYY-MM-DD (ISO date string without time)
 *
 * @param date - The date to format
 * @returns ISO date string (YYYY-MM-DD)
 */
export function formatDateISO(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/**
 * Parse a date string (YYYY-MM-DD) into a Date object
 *
 * @param dateString - Date string in YYYY-MM-DD format
 * @returns Normalized Date object, or null if invalid
 */
export function parseDateString(dateString: string): Date | null {
  const [year, month, day] = dateString.split('-').map(Number);

  // Validate that we got three numeric values
  if (!year || !month || !day) {
    return null;
  }

  // Create date (month is 0-indexed in Date constructor)
  const date = new Date(year, month - 1, day);

  // Validate that the date is semantically valid
  // (e.g., reject 2025-02-30 which JavaScript would auto-correct)
  if (date.getFullYear() !== year || date.getMonth() !== month - 1 || date.getDate() !== day) {
    return null;
  }

  return normalizeDate(date);
}

/**
 * Validate if a string is a valid YYYY-MM-DD date
 * Checks both syntax (regex) and semantics (valid calendar date)
 *
 * @param dateString - String to validate
 * @returns true if valid date, false otherwise
 */
export function isValidDateString(dateString: string): boolean {
  // First check syntax with regex
  const dateRegex = /^\d{4}-\d{2}-\d{2}$/;
  if (!dateRegex.test(dateString)) {
    return false;
  }

  // Then check semantics by parsing
  const parsed = parseDateString(dateString);
  return parsed !== null;
}
