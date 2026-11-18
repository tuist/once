require "minitest/autorun"

class StringTest < Minitest::Test
  def test_upcase
    assert_equal "STRUO", "struo".upcase
  end
end
