
import { formAction$ } from 'forms';
const x = 'module-level';
export const action = formAction$((data) => {
  let i = 0;
  do {
    const x = 'shadowed';
    i++;
  } while (i < 3);
  return {};
});
		