
		import { component$, useSignal, $ } from "@qwik.dev/core";

		export const Test = component$(() => {
			const sig = useSignal(0);
			const foo = useSignal('foo');
			const bar = useSignal('bar');
			return <button onClick$={() => sig.value++} onDblClick$={() => foo.value+=sig.value} onHover$={() => sig.value+=bar.value}>{sig}</button>;
		});
		