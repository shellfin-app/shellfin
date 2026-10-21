/** Shellfin brand mark (assets/brand/shellfin-logo.svg), inline so it needs no fetch and stays CSP-safe. */
export function BrandMark({ size = 22 }: { size?: number }) {
  return (
    <svg
      className="brand-mark"
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 512 512"
      aria-hidden="true"
      focusable="false"
    >
      <rect width="512" height="512" rx="112" fill="#4FD1C5" />
      <path d="M150 360 C 216 306 256 188 340 110 C 324 206 332 298 380 360 Z" fill="#0F1115" />
      <rect x="96" y="384" width="320" height="28" rx="14" fill="#0F1115" />
    </svg>
  );
}
