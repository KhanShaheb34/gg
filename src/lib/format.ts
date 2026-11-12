/** Abbreviated commit hash for display: the first 8 characters. */
export function shortHash(hash: string): string {
  return hash.slice(0, 8);
}
