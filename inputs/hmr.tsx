
		import { component$, component$, $ } from "@qwik.dev/core";

		export const TestGetsHmr = component$(() => {
			return <div>Test</div>;
		});
		export const TestNoHmr = componentQrl($(() => {
			return <div>Test</div>;
		}));
		