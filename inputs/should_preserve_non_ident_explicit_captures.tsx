
import { _captures, inlinedQrl } from '@qwik.dev/core';

const left = 1;
const right = 2;

export const task = inlinedQrl(() => {
	const left = _captures[0];
	const middle = _captures[1];
	const right = _captures[2];
	return middle ? left : right;
}, 'task', [left, true, right]);
