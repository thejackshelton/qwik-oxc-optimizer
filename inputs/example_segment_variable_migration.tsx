
import { component$ } from '@qwik.dev/core';

// This helper is only used by App component, so it should be migrated to its segment
const helperFn = (msg) => {
	console.log('Helper: ' + msg);
	return msg.toUpperCase();
};

// This shared variable is used by multiple segments, so it should stay at root
const SHARED_CONFIG = { value: 42 };

// This is an export, so it must stay at root
export const publicHelper = () => console.log('public');

export const App = component$(() => {
	const result = helperFn('hello');
	return <div>{result} {SHARED_CONFIG.value}</div>;
});

export const Other = component$(() => {
	return <div>{SHARED_CONFIG.value}</div>;
});
