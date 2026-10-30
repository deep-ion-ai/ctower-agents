const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** Bytes para pessoas, em base 1024 com uma casa decimal ("512 B", "1,5 KB", "6,6 GB"), como o `ctower clean`. */
export function humanBytes(b: number): string {
  if (b < 1024) return `${Math.max(0, Math.round(b))} B`;
  let v = b;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(1).replace(".", ",")} ${UNITS[u]}`;
}
