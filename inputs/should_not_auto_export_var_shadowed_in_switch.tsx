
import { formAction$ } from 'forms';
const x = 'module-level';
export const action = formAction$((data) => {
  switch (data.kind) {
    case 'a': {
      const x = 'shadowed-in-block';
      return x;
    }
    case 'b': {
      const x = 'shadowed-in-case';
      return x;
    }
  }
  return x;
});
		