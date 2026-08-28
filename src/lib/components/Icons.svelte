<!-- Named SVG icons — Lucide-style 16×16, currentColor stroke -->
<script lang="ts">
  export type IconName =
    | 'home' | 'folder' | 'search' | 'activity'
    | 'scan' | 'zap' | 'settings' | 'gear' | 'wifi' | 'wifi-off'
    | 'server' | 'globe' | 'chevron-right';

  const { name, size = 16, class: cls = '' } = $props<{
    name: IconName;
    size?: number;
    class?: string;
  }>();
</script>

{#if name === 'home'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <path d="M2 6.5L8 2l6 4.5V14a1 1 0 01-1 1H3a1 1 0 01-1-1V6.5z"/>
    <path d="M6 15V9h4v6"/>
  </svg>
{:else if name === 'folder'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <path d="M1 4a1 1 0 011-1h4l2 2h6a1 1 0 011 1v7a1 1 0 01-1 1H2a1 1 0 01-1-1V4z"/>
  </svg>
{:else if name === 'search'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <circle cx="7" cy="7" r="5"/>
    <path d="M14 14l-3-3"/>
  </svg>
{:else if name === 'activity'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <polyline points="1,9 4,5 7,11 10,3 13,7 15,7"/>
  </svg>
{:else if name === 'scan'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <rect x="1" y="1" width="5" height="5" rx="1"/>
    <rect x="10" y="1" width="5" height="5" rx="1"/>
    <rect x="1" y="10" width="5" height="5" rx="1"/>
    <path d="M10 13.5h5M13.5 10v5M10 10l1.5 1.5"/>
  </svg>
{:else if name === 'zap'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <polygon points="9,1 2,9 8,9 7,15 14,7 8,7 9,1"/>
  </svg>
{:else if name === 'settings'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <circle cx="8" cy="8" r="2.5"/>
    <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41"/>
  </svg>
{:else if name === 'gear'}
  <!-- Un vrai engrenage, à dents. L'icône `settings` ci-dessus est un cercle
       entouré de rayons : c'est le symbole universel du mode clair, et elle a
       été prise pour un bouton de thème là où elle ouvrait des actions
       destructrices. Deux usages, deux dessins. -->
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <circle cx="12" cy="12" r="3"/>
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
  </svg>
{:else if name === 'wifi'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <path d="M1.5 6C3.5 4 5.6 3 8 3s4.5 1 6.5 3"/>
    <path d="M4 9c1.1-1.1 2.4-1.7 4-1.7s2.9.6 4 1.7"/>
    <circle cx="8" cy="13" r="1" fill="currentColor" stroke="none"/>
  </svg>
{:else if name === 'wifi-off'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <path d="M1 1l14 14"/>
    <path d="M9.9 4.2C9.3 3.8 8.7 3.5 8 3.5c-2.4 0-4.5 1-6.5 3"/>
    <path d="M10.7 8.4c.5.4.9.8 1.3 1.3"/>
    <path d="M4 9c1.1-1.1 2.4-1.7 4-1.7.4 0 .8 0 1.2.1"/>
    <circle cx="8" cy="13" r="1" fill="currentColor" stroke="none"/>
  </svg>
{:else if name === 'server'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <rect x="1" y="2" width="14" height="5" rx="1"/>
    <rect x="1" y="9" width="14" height="5" rx="1"/>
    <circle cx="4.5" cy="4.5" r=".75" fill="currentColor" stroke="none"/>
    <circle cx="4.5" cy="11.5" r=".75" fill="currentColor" stroke="none"/>
  </svg>
{:else if name === 'globe'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <circle cx="8" cy="8" r="7"/>
    <path d="M1 8h14M8 1c-2 2-3 4.5-3 7s1 5 3 7M8 1c2 2 3 4.5 3 7s-1 5-3 7"/>
  </svg>
{:else if name === 'chevron-right'}
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class={cls}>
    <polyline points="6,3 11,8 6,13"/>
  </svg>
{/if}
