<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import Icon from './Icon.svelte';
  let {
    title,
    onclose,
    busy = false,
    children,
  }: {
    title: string;
    onclose: () => void;
    busy?: boolean;
    children: Snippet;
  } = $props();
  let element: HTMLDialogElement;
  let opener: Element | null;
  function keepFocus(event: KeyboardEvent) {
    if (event.key !== 'Tab') return;
    const controls = Array.from(
      element.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]',
      ),
    ).filter((control) => control.getClientRects().length > 0);
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
  onMount(() => {
    opener = document.activeElement;
    element.showModal();
    return () => {
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  });
</script>

<dialog
  bind:this={element}
  class="modal"
  aria-label={title}
  onkeydown={keepFocus}
  oncancel={(event) => {
    event.preventDefault();
    if (!busy) onclose();
  }}
>
  <header class="modal-heading">
    <h2>{title}</h2>
    <button
      class="icon-button"
      aria-label="Close dialog"
      disabled={busy}
      onclick={onclose}><Icon name="close" /></button
    >
  </header>
  {@render children()}
</dialog>
