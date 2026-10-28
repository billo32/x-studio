import { mount } from "svelte";
import "@fontsource-variable/literata";
import "./app.css";
import App from "./App.svelte";

export default mount(App, { target: document.getElementById("app") });
