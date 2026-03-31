
import { $, component$, server$, useStyle$, useTask$, useSignal } from '@qwik.dev/core';

export const Works = component$((props) => {
	useStyle$(STYLES);
	const text = 'hola';
	const sig = useSignal('hola');
	useTask$(() => {
		console.log(sig.value, text);
	});
	return (
		<div onClick$={server$(() => console.log('in server', sig.value, text))}></div>
	);
});

const STYLES = '.class {}';
