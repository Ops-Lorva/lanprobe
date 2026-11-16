<script lang="ts">
  import type { Snippet } from 'svelte';
  import { setScreenKind, type ScreenKind } from '../screen';

  let { kind, children }: { kind: ScreenKind; children: Snippet } = $props();
  // La portée ne change pas de nature : changer de disposition recrée l'arbre.
  // svelte-ignore state_referenced_locally
  setScreenKind(kind);
</script>

{@render children()}
