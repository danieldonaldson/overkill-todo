import type {
  AddTodoRequest,
  CreateListRequest,
  DoneTodoPatch,
  List,
  ListDetail,
  RenameListRequest,
  TitleTodoPatch,
  Todo,
} from '../types'
import { HttpClient } from './client'

// Lists and todos (OT-013). Every path has parameters in a PATCH/GET route, so
// `route` carries the template for browser error reports (design D9).
export function createListsApi(client: HttpClient) {
  return {
    /** The caller's lists, oldest first. */
    list: (): Promise<List[]> => client.request('/lists', { method: 'GET', route: '/lists' }),

    /** Create a list; a repeated `key` returns the first one. */
    create: (body: CreateListRequest): Promise<List> =>
      client.request('/lists', {
        method: 'POST',
        body: JSON.stringify(body),
        route: '/lists',
      }),

    /** One list with all its todos in contract order. */
    get: (id: string): Promise<ListDetail> =>
      client.request(`/lists/${id}`, { method: 'GET', route: '/lists/{id}' }),

    /** Rename a list at the version the Member saw. */
    rename: (id: string, body: RenameListRequest): Promise<List> =>
      client.request(`/lists/${id}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
        route: '/lists/{id}',
      }),

    /** Delete a list, confirmed by its version and todo count. */
    deleteList: (id: string, version: number, todoCount: number): Promise<void> =>
      client.request(`/lists/${id}?version=${version}&todo_count=${todoCount}`, {
        method: 'DELETE',
        route: '/lists/{id}',
      }),

    /** Add a todo at the bottom; a repeated `key` returns the first one. */
    addTodo: (id: string, body: AddTodoRequest): Promise<Todo> =>
      client.request(`/lists/${id}/todos`, {
        method: 'POST',
        body: JSON.stringify(body),
        route: '/lists/{id}/todos',
      }),

    /** Mark a todo done or not done at the version the Member saw. */
    markTodo: (id: string, todoId: string, body: DoneTodoPatch): Promise<Todo> =>
      client.request(`/lists/${id}/todos/${todoId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
        route: '/lists/{id}/todos/{todo_id}',
      }),

    /** Rename a todo at the version the Member saw. */
    renameTodo: (id: string, todoId: string, body: TitleTodoPatch): Promise<Todo> =>
      client.request(`/lists/${id}/todos/${todoId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
        route: '/lists/{id}/todos/{todo_id}',
      }),

    /** Delete a todo at the version the Member saw. */
    deleteTodo: (id: string, todoId: string, version: number): Promise<void> =>
      client.request(`/lists/${id}/todos/${todoId}?version=${version}`, {
        method: 'DELETE',
        route: '/lists/{id}/todos/{todo_id}',
      }),
  }
}
