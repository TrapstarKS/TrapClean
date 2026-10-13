<script lang="ts">
  import type { Entry, EntryPage } from '../types';
  import { bytes } from '../utils';
  let {
    page,
    onopen,
    onother,
  }: { page: EntryPage; onopen: (entry: Entry) => void; onother: () => void } =
    $props();
  type Tile = {
    entry: Entry | null;
    size: number;
    name: string;
    count: number;
    x: number;
    y: number;
    w: number;
    h: number;
  };
  function layout(page: EntryPage): Tile[] {
    const values: Tile[] = page.items
      .filter((item) => item.logicalBytes > 0)
      .map((entry) => ({
        entry,
        size: entry.logicalBytes,
        name: entry.name,
        count: 1,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
      }));
    const remainder = Math.max(
      0,
      page.totalBytes - values.reduce((sum, item) => sum + item.size, 0),
    );
    if (remainder > 0)
      values.push({
        entry: null,
        size: remainder,
        name: 'Other',
        count: Math.max(0, page.total - page.items.length),
        x: 0,
        y: 0,
        w: 0,
        h: 0,
      });
    values.sort((a, b) => b.size - a.size);
    const output: Tile[] = [];
    function split(items: Tile[], x: number, y: number, w: number, h: number) {
      if (!items.length) return;
      if (items.length === 1) {
        output.push({ ...items[0], x, y, w, h });
        return;
      }
      const total = items.reduce((sum, item) => sum + item.size, 0);
      let left = items[0].size;
      let cut = 1;
      while (
        cut < items.length - 1 &&
        Math.abs(left + items[cut].size - total / 2) <
          Math.abs(left - total / 2)
      ) {
        left += items[cut].size;
        cut++;
      }
      const ratio = left / total;
      if (w >= h * 1.5) {
        split(items.slice(0, cut), x, y, w * ratio, h);
        split(items.slice(cut), x + w * ratio, y, w * (1 - ratio), h);
      } else {
        split(items.slice(0, cut), x, y, w, h * ratio);
        split(items.slice(cut), x, y + h * ratio, w, h * (1 - ratio));
      }
    }
    split(values, 0, 0, 100, 100);
    return output;
  }
  const tiles = $derived(layout(page));
</script>

<div class="treemap" aria-label="Storage map, sized by logical bytes">
  {#each tiles as tile, index}
    <button
      class="map-tile"
      class:map-other={!tile.entry}
      style={`left:${tile.x}%;top:${tile.y}%;width:${tile.w}%;height:${tile.h}%;--tile:${index % 6}`}
      title={`${tile.name}: ${bytes(tile.size)}${tile.entry ? '' : ` across ${tile.count.toLocaleString()} remaining items`}`}
      aria-label={`${tile.name}, ${bytes(tile.size)}${tile.entry?.kind === 'directory' ? ', open folder' : ''}`}
      onclick={() => (tile.entry ? onopen(tile.entry) : onother())}
    >
      {#if tile.w * tile.h > 270}<span class="map-name">{tile.name}</span><span
          class="map-size">{bytes(tile.size)}</span
        >{/if}
    </button>
  {/each}
  {#if !tiles.length}<div class="map-empty">
      No sized entries in this folder.
    </div>{/if}
</div>
