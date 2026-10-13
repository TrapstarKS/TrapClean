export function bytes(value: number): string {
  if (!Number.isFinite(value)) return 'Unavailable';
  const sign = value < 0 ? '−' : '';
  const amount = Math.abs(value);
  if (amount < 1024) return `${sign}${amount.toLocaleString()} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  const exponent = Math.min(
    Math.floor(Math.log(amount) / Math.log(1024)),
    units.length,
  );
  return `${sign}${(amount / 1024 ** exponent).toLocaleString(undefined, { maximumFractionDigits: 1 })} ${units[exponent - 1]}`;
}
export function date(value: number | null): string {
  return value === null
    ? 'Not reported'
    : new Date(value * 1000).toLocaleString(undefined, {
        dateStyle: 'medium',
        timeStyle: 'short',
      });
}
export function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
export function disposition(value: string): string {
  if (
    value === 'candidate' ||
    value === 'cleanup' ||
    value === 'candidate_for_cleanup'
  )
    return 'Candidate for cleanup';
  if (value === 'keep') return 'Keep';
  return 'Review required';
}
export function shortRoot(path: string): string {
  return (
    path
      .replace(/[\\/]+$/, '')
      .split(/[\\/]/)
      .pop() || path
  );
}
