export function formatIdentifier(value: string): string {
  const words = value.replace(/_/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function formatRuntimeTime(value?: number): string {
  if (value === undefined || !Number.isFinite(value)) {
    return "Not observed";
  }
  return new Date(value).toLocaleString();
}
