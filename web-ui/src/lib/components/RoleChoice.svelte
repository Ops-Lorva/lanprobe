<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { ROLES, type Role } from '$lib/api';

  interface Props {
    value: Role;
    onchange: (role: Role) => void;
    /** Identifiant du groupe de boutons radio — deux formulaires peuvent coexister. */
    group: string;
  }
  let { value, onchange, group }: Props = $props();
</script>

<!--
  Ce que chaque rôle peut, **au moment de choisir**, sur la ligne du choix.

  Un paragraphe d'aide au-dessus des trois options oblige à le lire d'abord et à
  se rappeler ensuite lequel disait quoi ; ici la phrase est attachée à
  l'option, donc elle se lit en même temps qu'on la sélectionne. C'est aussi ce
  qui permet de comparer les trois d'un seul regard vertical.

  Boutons radio et non liste déroulante : trois choix mutuellement exclusifs
  dont le libellé ne suffit pas — « opérateur » ne dit pas qu'il peut révoquer
  une sonde. Une liste déroulante cacherait deux options sur trois, donc les
  deux tiers de la comparaison.
-->
<fieldset class="roles">
  <legend class="lp-sr">{$_('accounts.role')}</legend>
  {#each ROLES as r (r)}
    <label class="opt" class:on={value === r}>
      <input
        type="radio"
        name={group}
        value={r}
        checked={value === r}
        onchange={() => onchange(r)}
      />
      <span class="text">
        <span class="nm">{$_(`accounts.role_${r}`)}</span>
        <span class="can">{$_(`accounts.can_${r}`)}</span>
      </span>
    </label>
  {/each}
</fieldset>

<style>
  .roles {
    border: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .opt {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 9px 11px;
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    background: var(--ep-bg-tertiary);
    cursor: pointer;
    min-width: 0;
  }
  .opt:hover {
    border-color: var(--ep-border-strong);
  }
  /* L'option retenue porte l'accent : sur trois cadres identiques, seule la
     couleur dit lequel partira au hub. */
  .opt.on {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
  }
  .opt input {
    margin-top: 2px;
    accent-color: var(--ep-accent);
    width: 15px;
    height: 15px;
    flex-shrink: 0;
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .nm {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ep-text-primary);
  }
  .can {
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--ep-text-secondary);
  }
</style>
