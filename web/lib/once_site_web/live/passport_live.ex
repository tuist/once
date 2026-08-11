defmodule OnceSiteWeb.PassportLive do
  @moduledoc false

  use OnceSiteWeb, :live_view
  use Noora

  alias Noora.Button
  alias Noora.Filter
  alias OnceSite.Passport
  alias OnceSiteWeb.Layouts
  alias OnceSiteWeb.ZeroToOnceOgImage

  @page_size 3

  @impl true
  def mount(_params, session, socket) do
    {:ok,
     socket
     |> assign(available_filters: available_filters(), submission_error: nil)
     |> assign(:available_repositories, repositories_from_session(session))}
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
     |> assign(
       :meta_description,
       "Bring your open source repository to Once. Share it, climb the queue, and build faster."
     )
     |> assign(:og_image, ZeroToOnceOgImage.url())
     |> assign(:og_image_alt, "Zero-to-Once, the open source migration queue for Once")
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

  def handle_event("submit_repository", %{"repository" => repository}, socket) do
    case Enum.find(socket.assigns.available_repositories, &(&1["full_name"] == repository)) do
      nil ->
        {:noreply,
         assign(socket, :submission_error, "Select a repository from your GitHub account.")}

      attributes ->
        with {:ok, project} <- Passport.submit_authorized_repository(attributes),
             {:ok, _request} <- Passport.request_integration(project) do
          {:noreply,
           push_navigate(
             socket,
             to: "/github.com/#{project.github_account}/#{project.github_repository}/integrate"
           )}
        else
          _ ->
            {:noreply,
             assign(socket, :submission_error, "We could not add that repository to the queue.")}
        end
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
              Submit your repository. We will inspect it, queue the migration, and help it run with Once.
            </p>
          </div>
        </header>

        <div :if={@available_repositories == []} data-part="zero-to-once-submission">
          <strong>Bring your repository to Once</strong>
          <p>Log in with GitHub to choose from the repositories you can access.</p>
          <Button.button
            label="Log in with GitHub"
            href="/zero-to-once/github"
            variant="primary"
            size="medium"
          />
        </div>

        <form
          :if={@available_repositories != []}
          id="zero-to-once-submission"
          data-part="zero-to-once-submission"
          phx-submit="submit_repository"
        >
          <label for="repository">Choose a repository</label>
          <div>
            <select id="repository" name="repository" required>
              <option value="">Select a repository</option>
              <option :for={repository <- @available_repositories} value={repository["full_name"]}>
                {repository["full_name"]}
              </option>
            </select>
            <Button.button label="Add to the queue" variant="primary" size="medium" />
          </div>
          <p>
            Private repositories stay private. Public open source repositories may appear in the queue.
          </p>
          <p :if={@submission_error} role="alert">{@submission_error}</p>
        </form>

        <div data-part="zero-to-once-queue-heading">
          <div>
            <h2>Open source repositories in the queue</h2>
            <p>Confirmed open source repositories that are being evaluated for Once.</p>
          </div>
          <span>{@meta.total_count} repositories</span>
        </div>

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
              status={queue_badge_status(repository.integration_request.status)}
              label={queue_badge_label(repository.integration_request.status)}
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

  defp repositories_from_session(%{"zero_to_once_repository_key" => key}) do
    case Cachex.get(OnceSite.Passport.Cache, {:zero_to_once_repositories, key}) do
      {:ok, repositories} when is_list(repositories) -> repositories
      _ -> []
    end
  end

  defp repositories_from_session(_session), do: []

  defp queue_badge_status(:awaiting_access), do: "attention"
  defp queue_badge_status(:queued), do: "in_progress"
  defp queue_badge_status(:integrating), do: "success"

  defp queue_badge_label(:awaiting_access), do: "Awaiting access"
  defp queue_badge_label(:queued), do: "Queued"
  defp queue_badge_label(:integrating), do: "Migrating"
end
