<script lang="ts">
  // Radar logo mark for LanProbe — dark rounded square + indigo sweep.
  export let size: number = 24;
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 32 32"
  fill="none"
  xmlns="http://www.w3.org/2000/svg"
  class="logo-mark-svg"
  aria-label="LanProbe"
  role="img"
>
  <defs>
    <linearGradient id="lpBg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#0E1424" />
      <stop offset="1" stop-color="#090D1A" />
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="32" height="32" rx="6" fill="url(#lpBg)" />
  <!-- rings -->
  <circle cx="16" cy="16" r="11.3" stroke="#6366f1" stroke-opacity="0.30" stroke-width="0.9" />
  <circle cx="16" cy="16" r="6.7"  stroke="#6366f1" stroke-opacity="0.22" stroke-width="0.9" />
  <!-- sweep wedge -->
  <path d="M16 16 L16 4.7 A11.3 11.3 0 0 1 24.9 9.05 Z" fill="#6366f1" fill-opacity="0.32" />
  <!-- sweep line -->
  <line x1="16" y1="16" x2="16" y2="4.7" stroke="#818cf8" stroke-width="1.4" stroke-linecap="round" />
  <!-- detected nodes -->
  <circle cx="22"    cy="11.33" r="1.7" fill="#818cf8" />
  <circle cx="11.33" cy="20.67" r="1.3" fill="#6366f1" />
  <circle cx="20.67" cy="21.33" r="1.3" fill="#ffffff" fill-opacity="0.82" />
  <!-- core -->
  <circle cx="16" cy="16" r="1.9" fill="#ffffff" />
</svg>

<style>
  .logo-mark-svg {
    flex-shrink: 0;
    display: block;
    border-radius: 6px;
  }
</style>
