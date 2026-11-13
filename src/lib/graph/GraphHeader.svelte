<!-- The graph's column headers, with a handle to resize each column but the description. -->
<script lang="ts">
  import Splitter from "../components/Splitter.svelte";
  import { GRAPH_MIN_AUTO_W } from "./geometry";
  import { NAMES, RIGHT_COLUMNS, type GraphColumns } from "./graphColumns.svelte";

  let { columns }: { columns: GraphColumns } = $props();
  const widths = $derived(columns.widths);
</script>

<div class="header" role="row" aria-rowindex={1} data-header>
  <div class="hcell lanes-h" class:narrow={widths.graph < GRAPH_MIN_AUTO_W} role="columnheader">
    <span class="hlabel">Graph</span>
    <Splitter label="Resize Graph column" edge="right" value={widths.graph} {...columns.resizer("graph", 1)} />
  </div>
  <div class="hcell" role="columnheader"><span class="hlabel">Description</span></div>
  {#each RIGHT_COLUMNS as key (key)}
    <div class="hcell" role="columnheader">
      <span class="hlabel">{NAMES[key]}</span>
      <Splitter label="Resize {NAMES[key]} column" edge="left" value={widths[key]} {...columns.resizer(key, -1)} />
    </div>
  {/each}
</div>

<style>
  .header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: grid;
    grid-template-columns: var(--cols);
    height: var(--header-h);
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.01em;
  }

  .hcell {
    position: relative;
    display: flex;
    align-items: center;
    min-width: 0;
    padding: 0 10px;
  }

  .hlabel {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .lanes-h {
    padding: 0 8px;
  }

  .narrow .hlabel {
    visibility: hidden;
  }
</style>
