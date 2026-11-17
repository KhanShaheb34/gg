import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { markLoadTimings, measureLoadCosts, startMark } from "./lib/perf/startMarks";

// Performance runs only: the page's scripts have loaded and run up to here.
startMark("page-main");

const target = document.getElementById("app");
if (!target) throw new Error("#app mount point missing from index.html");

const app = mount(App, { target });
startMark("page-mounted");
markLoadTimings();
measureLoadCosts();

export default app;
