import { writable } from 'svelte/store';

/**
 * Message de confirmation qui survit à un changement d'écran.
 *
 * Après une révocation, l'utilisateur quitte le détail pour revenir au parc :
 * s'il n'emporte rien, il n'a aucune confirmation que ses mesures sont bien
 * conservées — et c'est exactement le doute qui empêche de faire le ménage la
 * fois suivante.
 */
export const flash = writable<string>('');
