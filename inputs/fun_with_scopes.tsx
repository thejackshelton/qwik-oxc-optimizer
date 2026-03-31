
		import { inlinedQrl,$,component$,jsx,useStylesScoped$ } from '@qwik.dev/core';
			export default () => {
				const serverFnHash = globalThis.foo();
				const data = globalThis.bar();
				const qrl = inlinedQrl(null, serverFnHash, data.slice(1));
				console.log(qrl);
			}

			function Lifecycle(props, key, flags) {
				return component$(() => {
					return /* @__PURE__ */ jsx(Slot, {});
				})(props, key, flags);
			}
			export const rawFn = (event, element) => {
						handleFieldEvent(form, field, name, event, element, [
							"touched",
							"input"
						], getElementInput(element, field, type));
					}
			export function Field({ children, name, type, ...props }) {
				const { of: form } = props;
				const field = getFieldStore(form, name);
				return /* @__PURE__ */ jsx(Lifecycle, {
					store: field,
					...props,
					children: children(field, {
						name,
						autoFocus: isServer && !!field.error,
						ref: $((element) => {
							field.internal.elements.push(element);
						}),
						onInput$: $(rawFn),
					})
				}, name);
			}

			const STYLE_RED = `.container {background-color: red;}`;
			describe.each([])('$render.name: useStylesScoped', ({ render }) => {

				it('should render object style', async () => {
					const StyledComponent = component$(() => {
						const stylesScopedData = useStylesScoped$(STYLE_RED);
						const store = useStore({
							count: 10,
						});

						return (
							<button class={['container', `count-${store.count}`]} onClick$={() => store.count++}>
								Hello world
							</button>
						);
					});

					const { vNode, getStyles, document } = await render(<StyledComponent />, { debug });
				})
			})

			