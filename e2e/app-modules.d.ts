// The app's own modules, as the page loads them from the vite dev server: tests reach them
// with `page.evaluate(() => import("/src/…"))`. Typed loosely here: they are browser code,
// checked by svelte-check with the app's config, not by this one.
declare module "/src/*";
