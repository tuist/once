defmodule OnceSiteWeb.PassportLive do
  @moduledoc false

  use OnceSiteWeb, :live_view
  use Noora

  alias Noora.Button
  alias Noora.Filter
  alias OnceSite.Passport
  alias OnceSiteWeb.Layouts

  @page_size 3

  @impl true
  def mount(_params, _session, socket) do
    {:ok, assign(socket, available_filters: available_filters(), submission_error: nil)}
  end

  @impl true
  def handle_params(params, uri, socket) do
    active_filters =
      Filter.Operations.decode_filters_from_query(params, socket.assigns.available_filters)

    {repositories, meta} =
      params
      |> Map.put("filters", Filter.Operations.convert_filters_to_flop(active_filters))
      |> Map.put("page_size", @page_size)
      |> Passport.list_public_repositories()

    {:noreply,
     socket
     |> assign(:page_title, "Zero-to-Once")
     |> assign(:uri, URI.parse(uri))
     |> assign(:active_filters, active_filters)
     |> assign(:repositories, repositories)
     |> assign(:meta, meta)}
  end

  @impl true
  def handle_event("add_filter", %{"value" => filter_id}, socket) do
    query =
      filter_id
      |> Filter.Operations.add_filter_to_query(socket)
      |> Map.delete("page")

    {:noreply, push_patch(socket, to: zero_to_once_path(query))}
  end

  def handle_event("update_filter", params, socket) do
    query =
      params
      |> Filter.Operations.update_filters_in_query(socket)
      |> Map.delete("page")

    {:noreply, push_patch(socket, to: zero_to_once_path(query))}
  end

  def handle_event("submit_repository", %{"repository_url" => repository_url}, socket) do
    case repository_path(repository_url) do
      {:ok, path} ->
        {:noreply, push_navigate(socket, to: path <> "/integrate")}

      :error ->
        {:noreply, assign(socket, :submission_error, "Enter a public GitHub repository URL.")}
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash}>
      <section data-part="passport-page">
        <header data-part="passport-header">
          <div>
            <h1>Zero-to-Once</h1>
            <p>
              Submit your open source project. We will inspect it, queue the migration, and help it run with Once.
            </p>
          </div>
        </header>

        <form
          id="zero-to-once-submission"
          data-part="zero-to-once-submission"
          phx-submit="submit_repository"
        >
          <label for="repository_url">Your public GitHub repository</label>
          <div>
            <input
              id="repository_url"
              name="repository_url"
              type="url"
              placeholder="https://github.com/account/repository"
              required
            />
            <Button.button label="Join the queue" variant="primary" size="medium" />
          </div>
          <p :if={@submission_error} role="alert">{@submission_error}</p>
        </form>

        <div data-part="passport-directory-controls">
          <.inline_dropdown
            id="passport-filter-dropdown"
            label="Filter repositories"
            on_select="add_filter"
          >
            <:icon><.filter /></:icon>
            <.dropdown_item
              :for={filter <- available_filters(@available_filters, @active_filters)}
              value={filter.id}
              label={filter.display_name}
            />
          </.inline_dropdown>
          <div :if={Enum.any?(@active_filters)} data-part="passport-active-filters">
            <.active_filter :for={filter <- @active_filters} filter={filter} />
          </div>
        </div>

        <div data-part="passport-directory">
          <a
            :for={repository <- @repositories}
            data-part="passport-directory-item"
            href={"/github.com/#{repository.github_account}/#{repository.github_repository}"}
          >
            <div>
              <strong>{repository.github_account}/{repository.github_repository}</strong>
              <span>{repository.github_description}</span>
            </div>
            <.status_badge
              status={if repository.scans == [], do: "in_progress", else: "success"}
              label={if repository.scans == [], do: "Indexing", else: "Indexed"}
            />
          </a>
        </div>

        <p :if={Enum.empty?(@repositories)} data-part="passport-directory-empty">
          No repositories match these filters.
        </p>

        <.pagination_group
          :if={@meta.total_pages > 1}
          current_page={@meta.current_page}
          number_of_pages={@meta.total_pages}
          page_patch={fn page -> zero_to_once_path(Map.put(query_params(@uri), "page", page)) end}
        />
      </section>
    </Layouts.app>
    """
  end

  defp available_filters do
    [
      %Filter.Filter{
        id: "github_account",
        field: :github_account,
        display_name: "Account",
        type: :text,
        operator: :=~
      },
      %Filter.Filter{
        id: "github_repository",
        field: :github_repository,
        display_name: "Repository",
        type: :text,
        operator: :=~
      }
    ]
  end

  defp available_filters(filters, active_filters) do
    active_filter_ids = MapSet.new(active_filters, & &1.id)
    Enum.reject(filters, &MapSet.member?(active_filter_ids, &1.id))
  end

  defp query_params(%URI{query: nil}), do: %{}
  defp query_params(%URI{query: query}), do: URI.decode_query(query)

  defp zero_to_once_path(query) do
    case URI.encode_query(query) do
      "" -> "/zero-to-once/"
      encoded_query -> "/zero-to-once/?#{encoded_query}"
    end
  end

  defp repository_path(repository_url) do
    with %URI{host: "github.com", path: path} <- URI.parse(String.trim(repository_url)),
         [account, repository] <- path |> String.split("/", trim: true) |> Enum.take(2),
         true <- account != "" and repository != "" do
      {:ok, "/github.com/#{account}/#{String.replace_suffix(repository, ".git", "")}"}
    else
      _ -> :error
    end
  end
end
