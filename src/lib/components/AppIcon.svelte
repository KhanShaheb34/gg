<!--
  gg's app icon, the real artwork: the bundle's icon as Windows and Linux show it
  (src-tauri/icons/src/app-icon-flat.svg, the macOS icon's tile and mark without the Dock
  grid's margin), drawn from its vector so it is sharp at any scale, the same in both themes.
-->
<script lang="ts">
  import icon from "../../../src-tauri/icons/src/app-icon-flat.svg";

  let { size = 48 }: { size?: number } = $props();
</script>

<img class="app-icon" src={icon} width={size} height={size} alt="" draggable="false" />

<style>
  .app-icon {
    display: block;
    flex: none;
    /* The tile's own edge carries it on dark; a soft shadow lifts it off light. */
    filter: drop-shadow(0 1px 1.5px rgba(16, 18, 27, 0.18));
  }
</style>
