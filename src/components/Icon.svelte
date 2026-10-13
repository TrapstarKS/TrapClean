<script lang="ts">
  let { name, size = 18 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    storage: 'M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01M11 7h6M11 17h6',
    clean: 'm14 3 7 7M17.5 6.5 9 15M4 12l8 8M4 12l-1 7 2 2 7-1M15 17h6M17 21h4',
    settings:
      'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M10 3h4l.7 2.5 2 .9 2.4-.7 2 3.5-1.7 1.8v2l1.7 1.8-2 3.5-2.4-.7-2 .9L14 21h-4l-.7-2.5-2-.9-2.4.7-2-3.5 1.7-1.8v-2L2.9 9.2l2-3.5 2.4.7 2-.9z',
    folder: 'M3 6h7l2 2h9v12H3zM3 6V4h7l2 2',
    file: 'M14 3H5v18h14V8zM14 3v5h5M8 13h8M8 17h5',
    arrow: 'M5 12h14m-5-5 5 5-5 5',
    chevron: 'm9 5 7 7-7 7',
    down: 'm6 9 6 6 6-6',
    back: 'm14 6-6 6 6 6',
    search: 'M10.5 3a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15m5.5 13 5 5',
    close: 'm6 6 12 12M6 18 18 6',
    check: 'm5 12 4 4L19 6',
    warning: 'm12 3 10 18H2zM12 9v5M12 17h.01',
    info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18M12 11v6M12 7h.01',
    trash: 'M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7',
    refresh:
      'M20 7v5h-5M4 17v-5h5M5 7a8 8 0 0 1 13-2l2 3M4 16l2 3a8 8 0 0 0 13-2',
    spark: 'm12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5z',
    copy: 'M9 8h12v13H9zM15 8V3H3v13h6',
    shield: 'm12 3 8 3v6c0 5-8 9-8 9S4 17 4 12V6zM8 12l3 3 5-6',
    key: 'M8 3a5 5 0 1 0 0 10 5 5 0 0 0 0-10m4 9 9 9m-4-4 3-3m-6 0 3-3',
    history: 'M3 4v6h6M3 10a9 9 0 1 1 2 8M12 7v5l3 2',
    minus: 'M5 12h14',
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.7"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"><path d={paths[name] || paths.file} /></svg
>
