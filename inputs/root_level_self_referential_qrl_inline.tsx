
import { component$ } from '@qwik.dev/core';

// Root-level self-referential component
// The QRL should NOT be hoisted before the component function is defined
// or should use two-phase emission to avoid forward reference
export const Tree = component$((props) => {
	return (
		<div>
			{props.label}
			{props.children && props.children.map((child) => <Tree {...child} />)}
		</div>
	);
});
