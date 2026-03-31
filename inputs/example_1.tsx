
import { $, component, onRender } from '@qwik.dev/core';

export const renderHeader1 = $(() => {
	return (
		<div onClick={$((ctx) => console.log(ctx))}/>
	);
});
const renderHeader2 = component($(() => {
	console.log("mount");
	return render;
}));
