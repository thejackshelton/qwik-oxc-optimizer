
import { component$, useAsync$ } from '@qwik.dev/core';
		
// Component-level self-referential component
export const Foo = component$((props) => {
	const sig = useAsync$(async ({cleanup}) => {
		const timer = setInterval(() => {
			sig.value++;
		}, 1000);
		cleanup(() => clearInterval(timer));
		return 0;
	});
	const other = useAsync$(async ({cleanup}) => {
		const timer = setInterval(() => {
			other.value++;
		}, 900);
		cleanup(() => clearInterval(timer));
		return 0;
	});
	return (
		<div>
			{other.value}
		</div>
	);
});
