/**
 * Contact constants
 *
 * @module constants/contact
 */

/**
 * Where the Labs "Team synchronization" card sends someone who wants team
 * collaboration (ADR-084 §1). A compile-time constant: the card opens it through
 * `openUrl` and adds no identifier, account, database or version data to it.
 */
export const TEAM_COLLABORATION_CONTACT_URL = 'mailto:developer@nodespace.ai';
