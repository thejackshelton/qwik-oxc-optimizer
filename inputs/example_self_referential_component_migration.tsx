
import { component$ } from '@qwik.dev/core';

// Self-referential component: the Nested component references itself in its JSX
// This should be migrated to its segment using two-phase emission (let + assign)
// to avoid Temporal Dead Zone errors
export const Nested = component$(() => {
	return (
		<div>
			<Nested />
		</div>
	);
});

// Another self-referential component with conditional rendering
export const RecursiveList = component$((props) => {
	if (props.depth === 0) return <div>End</div>;
	return (
		<div>
			Level {props.depth}
			<RecursiveList depth={props.depth - 1} />
		</div>
	);
});

// Mutually recursive components: A references B, B references A
const ComponentA = component$(() => {
	return (
		<div>
			A
			<ComponentB />
		</div>
	);
});

const ComponentB = component$(() => {
	return (
		<div>
			B
			<ComponentA />
		</div>
	);
});

export const MutualExample = component$(() => {
	return <ComponentA />;
});
